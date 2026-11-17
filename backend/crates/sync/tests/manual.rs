//! Manual overrides, void and reopen (T05 acceptance criteria 1 and 2),
//! against a temp database with one data source named `pandascore` (the
//! real adapter arrives in T04; the reconciler only sees DTOs).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{
    DomainEvent, EntityType, MapStatus, MatchFormat, MatchId, MatchStatus, PickParts, Side,
};
use sources::fake::sourced;
use sources::{SrcEvent, SrcMap, SrcMatch, SrcTeam};
use store::SqlitePool;
use store::users::User;
use sync::{
    EventBus, MANUAL, ManualError, ManualService, Override, OverrideField, Precedence, Reconciler,
};
use tokio::sync::broadcast::Receiver;

const PS: &str = "pandascore";
const ADMIN: &str = "300000000000000001";
const GUILD: &str = "100000000000000001";

struct Env {
    pool: SqlitePool,
    rec: Arc<Reconciler>,
    manual: ManualService,
    rx: Receiver<DomainEvent>,
    _dir: tempfile::TempDir,
}

fn at(h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, h, m, 0).unwrap()
}

fn now() -> DateTime<Utc> {
    at(9, 0)
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("test.db").display());
    let pool = store::open(&url).await.unwrap();
    let bus = EventBus::new();
    let rx = bus.subscribe();
    let rec = Arc::new(Reconciler::new(
        pool.clone(),
        Precedence::from_enabled(&[PS.to_owned()]),
        Some(PS.to_owned()),
        bus,
    ));
    Env {
        pool,
        manual: ManualService::new(rec.clone()),
        rec,
        rx,
        _dir: dir,
    }
}

fn drain(rx: &mut Receiver<DomainEvent>) -> Vec<DomainEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

/// A Bo3 at 18:00, Vitality (A) vs NAVI (B), with whatever else `f` sets.
fn ps_match(f: impl FnOnce(&mut SrcMatch)) -> SrcMatch {
    let mut m = SrcMatch {
        team_a: Some(SrcTeam::named("t1", "Vitality")),
        team_b: Some(SrcTeam::named("t2", "NAVI")),
        format: Some(MatchFormat::Bo3),
        status: Some(MatchStatus::Scheduled),
        scheduled_at: Some(at(18, 0)),
        ..SrcMatch::new("m1", "e1")
    };
    f(&mut m);
    m
}

fn finished_2_1(m: &mut SrcMatch) {
    m.status = Some(MatchStatus::Finished);
    m.started_at = Some(at(18, 5));
    m.ended_at = Some(at(21, 0));
    m.score = Some((2, 1));
    m.maps = vec![SrcMap {
        name: Some("Inferno".into()),
        status: Some(MapStatus::Finished),
        rounds: Some((13, 7)),
        winner: Some(Side::A),
        ..SrcMap::new(1)
    }];
}

/// Ingests `dto` from `pandascore` and returns the canonical match ID.
async fn sync(e: &Env, dto: SrcMatch) -> MatchId {
    e.rec
        .ingest_event(PS, &sourced(&SrcEvent::new("e1", "IEM Test")), now())
        .await
        .unwrap();
    let rep = e
        .rec
        .ingest_matches(PS, &[sourced(&dto)], now())
        .await
        .unwrap();
    assert_eq!(rep.failed, 0);
    let id = store::refs::resolve(&e.pool, PS, EntityType::Match, "m1")
        .await
        .unwrap()
        .unwrap();
    MatchId(id)
}

async fn provenance(pool: &SqlitePool, id: MatchId) -> BTreeMap<String, String> {
    store::provenance::list_for(pool, EntityType::Match, id.0)
        .await
        .unwrap()
        .into_iter()
        .map(|f| (f.field, f.source))
        .collect()
}

async fn audit_actions(pool: &SqlitePool, id: MatchId) -> Vec<(String, String)> {
    store::audit::list_for_entity(pool, "match", &id.to_string())
        .await
        .unwrap()
        .into_iter()
        .map(|a| (a.record.actor, a.record.action))
        .collect()
}

/// AC: a manual score beats the source value and persists after a sync;
/// clearing restores the source value.
#[tokio::test]
async fn manual_scores_beat_the_source_survive_sync_and_clear_back() {
    let mut e = env().await;
    let id = sync(&e, ps_match(finished_2_1)).await;
    drain(&mut e.rx);

    // The source says 2:1 for A; the admin corrects it to 1:2.
    let out = e
        .manual
        .set(id, Override::Series { score: (1, 2) }, ADMIN, now())
        .await
        .unwrap();
    let f = &out.current.fields;
    assert_eq!((f.maps_won_a, f.maps_won_b), (1, 2));
    assert_eq!(f.winner_team_id, f.team_b_id, "winner follows the score");
    assert_eq!(
        out.events,
        vec![DomainEvent::ResultCorrected { match_id: id }]
    );
    assert_eq!(drain(&mut e.rx), out.events, "published on the bus");

    // A map override in canonical orientation.
    e.manual
        .set(
            id,
            Override::Map {
                number: 1,
                status: MapStatus::Finished,
                rounds: Some((11, 13)),
            },
            ADMIN,
            now(),
        )
        .await
        .unwrap();

    // The next sync reports a different source value: manual still wins.
    sync(
        &e,
        ps_match(|m| {
            finished_2_1(m);
            m.maps[0].rounds = Some((13, 8));
        }),
    )
    .await;
    let m = store::matches::get(&e.pool, id).await.unwrap().unwrap();
    assert_eq!((m.fields.maps_won_a, m.fields.maps_won_b), (1, 2));
    let maps = store::matches::list_maps(&e.pool, id).await.unwrap();
    assert_eq!((maps[0].rounds_a, maps[0].rounds_b), (Some(11), Some(13)));
    assert_eq!(maps[0].winner_team_id, m.fields.team_b_id);
    assert_eq!(maps[0].map_name.as_deref(), Some("Inferno"), "names stay");
    let prov = provenance(&e.pool, id).await;
    assert_eq!(prov["series"], MANUAL);
    assert_eq!(prov["map_rounds.1"], MANUAL);
    assert_eq!(prov["map_names.1"], PS);

    // Clearing falls back to the source values.
    e.manual
        .clear(id, OverrideField::Series, ADMIN, now())
        .await
        .unwrap();
    let out = e
        .manual
        .clear(id, OverrideField::Map(1), ADMIN, now())
        .await
        .unwrap();
    let f = &out.current.fields;
    assert_eq!((f.maps_won_a, f.maps_won_b), (2, 1));
    assert_eq!(f.winner_team_id, f.team_a_id);
    assert_eq!(
        (out.maps[0].rounds_a, out.maps[0].rounds_b),
        (Some(13), Some(8))
    );
    let prov = provenance(&e.pool, id).await;
    assert_eq!(prov["series"], PS);
    assert_eq!(prov["map_rounds.1"], PS);

    // With no override left, the manual record and link are gone.
    assert!(e.manual.overrides(id).await.unwrap().is_none());
    assert_eq!(
        store::refs::resolve(&e.pool, MANUAL, EntityType::Match, &id.to_string())
            .await
            .unwrap(),
        None
    );

    // Every change is audited with the Discord user.
    let actions = audit_actions(&e.pool, id).await;
    assert_eq!(
        actions,
        [
            "manual_override_set",
            "manual_override_set",
            "manual_override_clear",
            "manual_override_clear"
        ]
        .map(|a| (ADMIN.to_owned(), a.to_owned()))
    );
    let first = &store::audit::list_for_entity(&e.pool, "match", &id.to_string())
        .await
        .unwrap()[0]
        .record;
    assert_eq!(
        first.before,
        Some(serde_json::json!({ "field": "series", "value": null }))
    );
    assert_eq!(
        first.after,
        Some(serde_json::json!({ "field": "series", "value": { "score": [1, 2] } }))
    );

    // Clearing again is a no-op without an audit row.
    e.manual
        .clear(id, OverrideField::Series, ADMIN, now())
        .await
        .unwrap();
    assert_eq!(audit_actions(&e.pool, id).await.len(), 4);
}

#[tokio::test]
async fn clearing_without_another_source_resets_scores_to_neutral() {
    let mut e = env().await;
    // The source has no score and no maps yet.
    let id = sync(&e, ps_match(|_| {})).await;
    e.manual
        .set(id, Override::Series { score: (1, 0) }, ADMIN, now())
        .await
        .unwrap();
    e.manual
        .set(
            id,
            Override::Map {
                number: 2,
                status: MapStatus::Finished,
                rounds: Some((16, 14)),
            },
            ADMIN,
            now(),
        )
        .await
        .unwrap();
    drain(&mut e.rx);

    e.manual
        .clear(id, OverrideField::Series, ADMIN, now())
        .await
        .unwrap();
    let out = e
        .manual
        .clear(id, OverrideField::Map(2), ADMIN, now())
        .await
        .unwrap();
    let f = &out.current.fields;
    assert_eq!((f.maps_won_a, f.maps_won_b, f.winner_team_id), (0, 0, None));
    let map2 = out.maps.iter().find(|m| m.number == 2).unwrap();
    assert_eq!(
        (map2.status, map2.rounds_a, map2.winner_team_id),
        (MapStatus::Upcoming, None, None)
    );
    let prov = provenance(&e.pool, id).await;
    assert!(!prov.contains_key("series"), "{prov:?}");
    assert!(!prov.contains_key("map_rounds.2"), "{prov:?}");
    assert!(drain(&mut e.rx).contains(&DomainEvent::MapUpdated {
        match_id: id,
        number: 2
    }));

    // A later sync does not bring the old manual values back.
    sync(&e, ps_match(|_| {})).await;
    let m = store::matches::get(&e.pool, id).await.unwrap().unwrap();
    assert_eq!((m.fields.maps_won_a, m.fields.maps_won_b), (0, 0));
}

#[tokio::test]
async fn status_and_schedule_overrides() {
    let mut e = env().await;
    let id = sync(&e, ps_match(|_| {})).await;
    drain(&mut e.rx);

    let out = e
        .manual
        .set(
            id,
            Override::Schedule {
                scheduled_at: at(20, 0),
            },
            ADMIN,
            now(),
        )
        .await
        .unwrap();
    assert_eq!(out.current.fields.scheduled_at, Some(at(20, 0)));
    assert_eq!(out.current.fields.lock_at, Some(at(20, 0)), "lock follows");
    assert!(matches!(
        out.events[..],
        [DomainEvent::MatchRescheduled { .. }]
    ));

    // Live without timestamps: started now.
    let out = e
        .manual
        .set(
            id,
            Override::Status {
                status: MatchStatus::Live,
                started_at: None,
                ended_at: None,
            },
            ADMIN,
            at(20, 3),
        )
        .await
        .unwrap();
    assert_eq!(out.current.fields.status, MatchStatus::Live);
    assert_eq!(out.current.fields.started_at, Some(at(20, 3)));
    assert_eq!(out.events, vec![DomainEvent::MatchStarted { match_id: id }]);

    // Finished keeps the start and ends now.
    let out = e
        .manual
        .set(
            id,
            Override::Status {
                status: MatchStatus::Finished,
                started_at: None,
                ended_at: None,
            },
            ADMIN,
            at(22, 0),
        )
        .await
        .unwrap();
    assert_eq!(
        (out.current.fields.started_at, out.current.fields.ended_at),
        (Some(at(20, 3)), Some(at(22, 0)))
    );

    // Both survive a sync; clearing returns to the source's values.
    sync(&e, ps_match(|_| {})).await;
    let m = store::matches::get(&e.pool, id).await.unwrap().unwrap();
    assert_eq!(
        (m.fields.status, m.fields.scheduled_at),
        (MatchStatus::Finished, Some(at(20, 0)))
    );
    e.manual
        .clear(id, OverrideField::Status, ADMIN, now())
        .await
        .unwrap();
    let out = e
        .manual
        .clear(id, OverrideField::Schedule, ADMIN, now())
        .await
        .unwrap();
    assert_eq!(
        (
            out.current.fields.status,
            out.current.fields.scheduled_at,
            out.current.fields.started_at
        ),
        (MatchStatus::Scheduled, Some(at(18, 0)), None)
    );
}

#[tokio::test]
async fn invalid_overrides_are_rejected() {
    let e = env().await;
    let id = sync(&e, ps_match(|_| {})).await;
    let invalid = [
        Override::Series { score: (2, 2) },
        Override::Series { score: (3, 0) },
        Override::Map {
            number: 4,
            status: MapStatus::Finished,
            rounds: Some((13, 2)),
        },
        Override::Map {
            number: 1,
            status: MapStatus::Finished,
            rounds: Some((12, 12)),
        },
        Override::Map {
            number: 1,
            status: MapStatus::Finished,
            rounds: None,
        },
        Override::Map {
            number: 1,
            status: MapStatus::Upcoming,
            rounds: Some((3, 2)),
        },
    ];
    for ov in invalid {
        let err = e
            .manual
            .set(id, ov.clone(), ADMIN, now())
            .await
            .unwrap_err();
        assert!(matches!(err, ManualError::Invalid(_)), "{ov:?}: {err:?}");
    }
    assert!(matches!(
        e.manual
            .set(
                MatchId(999),
                Override::Series { score: (1, 0) },
                ADMIN,
                now()
            )
            .await,
        Err(ManualError::NotFound(MatchId(999)))
    ));
    assert!(matches!(
        e.manual
            .set(id, Override::Series { score: (1, 0) }, "not-a-user", now())
            .await,
        Err(ManualError::Invalid(_))
    ));
    // Nothing was written.
    assert!(e.manual.overrides(id).await.unwrap().is_none());
    assert!(audit_actions(&e.pool, id).await.is_empty());
}

#[tokio::test]
async fn scores_need_both_teams() {
    let e = env().await;
    let id = sync(
        &e,
        ps_match(|m| {
            m.team_b = None;
        }),
    )
    .await;
    let err = e
        .manual
        .set(id, Override::Series { score: (1, 0) }, ADMIN, now())
        .await
        .unwrap_err();
    assert!(matches!(err, ManualError::Conflict(_)), "{err:?}");
}

/// AC: void is audited, keeps picks, triggers re-settlement and survives
/// a sync; unvoid takes it back.
#[tokio::test]
async fn void_and_unvoid() {
    let mut e = env().await;
    let id = sync(&e, ps_match(finished_2_1)).await;
    let m = store::matches::get(&e.pool, id).await.unwrap().unwrap();
    store::users::upsert_user(
        &e.pool,
        &User {
            discord_user_id: "200000000000000001".into(),
            username: "alice".into(),
            global_name: None,
            avatar_hash: None,
            updated_at: now(),
        },
    )
    .await
    .unwrap();
    store::picks::save(
        &e.pool,
        GUILD,
        "200000000000000001",
        id,
        &PickParts {
            winner_team_id: m.fields.team_a_id,
            ..PickParts::default()
        },
        now(),
    )
    .await
    .unwrap();
    drain(&mut e.rx);

    let out = e.manual.void(id, ADMIN, now()).await.unwrap();
    assert!(out.current.fields.voided);
    assert_eq!(out.events, vec![DomainEvent::MatchVoided { match_id: id }]);
    assert_eq!(drain(&mut e.rx), out.events);
    assert!(matches!(
        e.manual.void(id, ADMIN, now()).await,
        Err(ManualError::Conflict(_))
    ));

    // Picks are kept; a sync does not undo the void.
    assert_eq!(
        store::picks::list_for_match(&e.pool, GUILD, id)
            .await
            .unwrap()
            .len(),
        1
    );
    sync(&e, ps_match(finished_2_1)).await;
    assert!(
        store::matches::get(&e.pool, id)
            .await
            .unwrap()
            .unwrap()
            .fields
            .voided
    );

    let out = e.manual.unvoid(id, ADMIN, now()).await.unwrap();
    assert!(!out.current.fields.voided);
    assert_eq!(
        out.events,
        vec![DomainEvent::MatchUnvoided { match_id: id }]
    );
    assert!(matches!(
        e.manual.unvoid(id, ADMIN, now()).await,
        Err(ManualError::Conflict(_))
    ));

    let audit = store::audit::list_for_entity(&e.pool, "match", &id.to_string())
        .await
        .unwrap();
    let rows: Vec<_> = audit
        .iter()
        .map(|a| {
            (
                a.record.actor.as_str(),
                a.record.action.as_str(),
                a.record.after.clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                ADMIN,
                "match_void",
                Some(serde_json::json!({ "voided": true }))
            ),
            (
                ADMIN,
                "match_unvoid",
                Some(serde_json::json!({ "voided": false }))
            ),
        ]
    );
}

/// AC: reopen only before the match is live, audited, triggers
/// re-settlement.
#[tokio::test]
async fn reopen_picks_after_a_postponement() {
    let mut e = env().await;
    let id = sync(&e, ps_match(|_| {})).await;
    let lock = async |pool: &SqlitePool| {
        let mut m = store::matches::get(pool, id).await.unwrap().unwrap();
        m.fields.locked = true;
        store::matches::update(pool, &m).await.unwrap();
    };
    let conflict = async |manual: &ManualService, at: DateTime<Utc>| {
        let err = manual.reopen(id, ADMIN, at).await.unwrap_err();
        assert!(matches!(err, ManualError::Conflict(_)), "{err:?}");
    };

    // Not locked yet.
    conflict(&e.manual, now()).await;

    // Locked at 18:00, then postponed without a new time: still in the past.
    lock(&e.pool).await;
    e.manual
        .set(
            id,
            Override::Status {
                status: MatchStatus::Postponed,
                started_at: None,
                ended_at: None,
            },
            ADMIN,
            at(18, 1),
        )
        .await
        .unwrap();
    conflict(&e.manual, at(18, 2)).await;

    // A new time in the future: reopen works and the lock follows it.
    let tomorrow = at(18, 0) + Duration::days(1);
    e.manual
        .set(
            id,
            Override::Schedule {
                scheduled_at: tomorrow,
            },
            ADMIN,
            at(18, 3),
        )
        .await
        .unwrap();
    let m = store::matches::get(&e.pool, id).await.unwrap().unwrap();
    assert_eq!(m.fields.lock_at, Some(at(18, 0)), "locked keeps lock_at");
    drain(&mut e.rx);
    let out = e.manual.reopen(id, ADMIN, at(18, 4)).await.unwrap();
    assert!(!out.current.fields.locked);
    assert_eq!(out.current.fields.lock_at, Some(tomorrow));
    assert_eq!(
        out.events,
        vec![DomainEvent::PicksReopened { match_id: id }]
    );
    assert_eq!(drain(&mut e.rx), out.events);
    let last = store::audit::list_for_entity(&e.pool, "match", &id.to_string())
        .await
        .unwrap()
        .pop()
        .unwrap()
        .record;
    assert_eq!(
        (last.actor.as_str(), last.action.as_str()),
        (ADMIN, "picks_reopen")
    );

    // Once live, it can't be reopened.
    lock(&e.pool).await;
    e.manual
        .set(
            id,
            Override::Status {
                status: MatchStatus::Live,
                started_at: None,
                ended_at: None,
            },
            ADMIN,
            at(18, 5),
        )
        .await
        .unwrap();
    conflict(&e.manual, at(18, 6)).await;
}
