//! Reconciler integration tests (T03 acceptance criteria) against a temp
//! database, with two fake sources: `alpha` (primary) and `beta`.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use domain::{
    DomainEvent, EntityType, EventId, EventTier, FeedOutcome, MapStatus, MatchFormat, MatchId,
    MatchStatus, Side, StageKind, TeamId, UnlinkedReason,
};
use sources::fake::{FakeAdapter, sourced};
use sources::{SourceAdapter, SrcBracket, SrcEvent, SrcFeed, SrcMap, SrcMatch, SrcStage, SrcTeam};
use store::SqlitePool;
use sync::reconcile::record;
use sync::{EventBus, FieldGroup, MANUAL, Precedence, Reconciler};
use tokio::sync::broadcast::Receiver;

// ---- fixtures ----------------------------------------------------------------

struct Db {
    pool: SqlitePool,
    _dir: tempfile::TempDir,
}

async fn db() -> Db {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("test.db").display());
    Db {
        pool: store::open(&url).await.unwrap(),
        _dir: dir,
    }
}

fn at(h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, h, m, 0).unwrap()
}

fn now() -> DateTime<Utc> {
    at(9, 0)
}

/// A reconciler with `alpha` as primary and `alpha, beta` enabled.
fn reconciler(pool: &SqlitePool, groups: &[(FieldGroup, &[&str])]) -> Reconciler {
    let enabled = vec!["alpha".to_owned(), "beta".to_owned()];
    let cfg: BTreeMap<FieldGroup, Vec<String>> = groups
        .iter()
        .map(|(g, v)| (*g, v.iter().map(|s| (*s).to_owned()).collect()))
        .collect();
    Reconciler::new(
        pool.clone(),
        Precedence::new(&enabled, &cfg).unwrap(),
        Some("alpha".to_owned()),
        EventBus::new(),
    )
}

fn team(ext: &str, name: &str, acronym: Option<&str>) -> SrcTeam {
    SrcTeam {
        acronym: acronym.map(Into::into),
        ..SrcTeam::named(ext, name)
    }
}

fn alpha_event() -> SrcEvent {
    SrcEvent {
        tier: Some(EventTier::S),
        stages: vec![SrcStage {
            ext_id: "a-s1".into(),
            name: "Playoffs".into(),
            kind: Some(StageKind::DoubleElim),
            position: 1,
        }],
        ..SrcEvent::new("a-e1", "IEM Test")
    }
}

fn beta_event() -> SrcEvent {
    SrcEvent {
        stages: vec![SrcStage {
            ext_id: "b-s1".into(),
            name: "playoffs".into(),
            kind: None,
            position: 0,
        }],
        ..SrcEvent::new("b-e1", "IEM Test 2026")
    }
}

fn a_vit() -> SrcTeam {
    team("a-vit", "Vitality", Some("VIT"))
}
fn a_navi() -> SrcTeam {
    team("a-navi", "Natus Vincere", Some("NAVI"))
}
fn b_vit() -> SrcTeam {
    team("b-vit", "Team Vitality", None)
}
fn b_navi() -> SrcTeam {
    team("b-navi", "NaVi Esports", Some("navi"))
}

/// Alpha's Vitality vs NAVI, Bo3 at 18:00, map 1 Inferno 13:10 for A.
fn alpha_m1() -> SrcMatch {
    SrcMatch {
        stage_ext_id: Some("a-s1".into()),
        team_a: Some(a_vit()),
        team_b: Some(a_navi()),
        format: Some(MatchFormat::Bo3),
        status: Some(MatchStatus::Scheduled),
        scheduled_at: Some(at(18, 0)),
        maps: vec![SrcMap {
            name: Some("Inferno".into()),
            rounds: Some((13, 10)),
            winner: Some(Side::A),
            ..SrcMap::new(1)
        }],
        ..SrcMatch::new("a-m1", "a-e1")
    }
}

/// Beta's view of the same match: 18:30, map 1 13:11.
fn beta_m1() -> SrcMatch {
    SrcMatch {
        team_a: Some(b_vit()),
        team_b: Some(b_navi()),
        scheduled_at: Some(at(18, 30)),
        maps: vec![SrcMap {
            rounds: Some((13, 11)),
            winner: Some(Side::A),
            ..SrcMap::new(1)
        }],
        ..SrcMatch::new("b-m1", "b-e1")
    }
}

/// Alpha's event and m1, beta's event linked by an admin. Returns the
/// canonical event and match.
async fn setup(r: &Reconciler, pool: &SqlitePool) -> (EventId, MatchId) {
    r.ingest_event("alpha", &sourced(&alpha_event()), now())
        .await
        .unwrap();
    r.ingest_matches("alpha", &[sourced(&alpha_m1())], now())
        .await
        .unwrap();
    let event = EventId(
        resolve(pool, "alpha", EntityType::Event, "a-e1")
            .await
            .unwrap(),
    );
    let rep = r
        .ingest_event("beta", &sourced(&beta_event()), now())
        .await
        .unwrap();
    assert_eq!(rep.queued, 1, "events of non-primary sources need a link");
    r.link("beta", EntityType::Event, "b-e1", event.0, "admin", now())
        .await
        .unwrap();
    let m1 = MatchId(
        resolve(pool, "alpha", EntityType::Match, "a-m1")
            .await
            .unwrap(),
    );
    (event, m1)
}

async fn resolve(pool: &SqlitePool, source: &str, t: EntityType, ext: &str) -> Option<i64> {
    store::refs::resolve(pool, source, t, ext).await.unwrap()
}

async fn count(pool: &SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn provenance(pool: &SqlitePool, t: EntityType, id: i64) -> BTreeMap<String, String> {
    store::provenance::list_for(pool, t, id)
        .await
        .unwrap()
        .into_iter()
        .map(|f| (f.field, f.source))
        .collect()
}

fn drain(rx: &mut Receiver<DomainEvent>) -> Vec<DomainEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

/// Every canonical row plus provenance, links and queue, for equality checks.
async fn snapshot(pool: &SqlitePool) -> Vec<String> {
    let mut out = Vec::new();
    for sql in [
        "SELECT json_group_array(json_array(id, name, acronym, logo_url, country)) FROM (SELECT * FROM teams ORDER BY id)",
        "SELECT json_group_array(json_array(id, name, tier, begin_at, end_at, status)) FROM (SELECT * FROM events ORDER BY id)",
        "SELECT json_group_array(json_array(id, event_id, name, kind, position)) FROM (SELECT * FROM stages ORDER BY id)",
        "SELECT json_group_array(json_array(id, event_id, stage_id, team_a_id, team_b_id, format, status, scheduled_at, started_at, ended_at, lock_at, locked, maps_won_a, maps_won_b, winner_team_id, bracket_side, round_index, slot_index, feeds_a_from_match_id, feeds_a_from_outcome, feeds_b_from_match_id, feeds_b_from_outcome)) FROM (SELECT * FROM matches ORDER BY id)",
        "SELECT json_group_array(json_array(match_id, number, map_name, status, rounds_a, rounds_b, winner_team_id)) FROM (SELECT * FROM maps ORDER BY match_id, number)",
        "SELECT json_group_array(json_array(entity_type, entity_id, field, source, updated_at)) FROM (SELECT * FROM field_sources ORDER BY 1, 2, 3)",
        "SELECT json_group_array(json_array(source, entity_type, external_id, entity_id)) FROM (SELECT * FROM external_refs ORDER BY 1, 2, 3)",
        "SELECT json_group_array(json_array(source, entity_type, external_id, reason, detail, first_seen_at)) FROM (SELECT * FROM unlinked_refs ORDER BY 1, 2, 3)",
    ] {
        out.push(
            sqlx::query_scalar::<_, String>(sql)
                .fetch_one(pool)
                .await
                .unwrap(),
        );
    }
    out
}

// ---- acceptance criteria -------------------------------------------------------

/// AC: two fake adapters that disagree resolve per the configured precedence.
#[tokio::test]
async fn disagreeing_sources_resolve_by_precedence() {
    let db = db().await;
    let alpha_first = [
        (FieldGroup::Schedule, &["manual", "alpha", "beta"][..]),
        (FieldGroup::MapRounds, &["manual", "beta", "alpha"][..]),
    ];
    let r = reconciler(&db.pool, &alpha_first);

    // The adapters return the DTOs; the reconciler only sees their output.
    let alpha = FakeAdapter::new("alpha");
    alpha.put_event(alpha_event());
    alpha.put_match(alpha_m1());
    let beta = FakeAdapter::new("beta");
    beta.put_event(beta_event());
    beta.put_match(beta_m1());

    r.ingest_event("alpha", &alpha.fetch_event("a-e1").await.unwrap(), now())
        .await
        .unwrap();
    r.ingest_matches(
        "alpha",
        &alpha.fetch_event_matches("a-e1").await.unwrap(),
        now(),
    )
    .await
    .unwrap();
    let event = resolve(&db.pool, "alpha", EntityType::Event, "a-e1")
        .await
        .unwrap();
    r.ingest_event("beta", &beta.fetch_event("b-e1").await.unwrap(), now())
        .await
        .unwrap();
    r.link("beta", EntityType::Event, "b-e1", event, "admin", now())
        .await
        .unwrap();
    let rep = r
        .ingest_matches(
            "beta",
            &beta.fetch_event_matches("b-e1").await.unwrap(),
            now(),
        )
        .await
        .unwrap();
    assert_eq!((rep.created, rep.linked, rep.queued), (0, 3, 0), "{rep:?}");

    let m1 = MatchId(
        resolve(&db.pool, "alpha", EntityType::Match, "a-m1")
            .await
            .unwrap(),
    );
    assert_eq!(
        resolve(&db.pool, "beta", EntityType::Match, "b-m1").await,
        Some(m1.0)
    );
    let m = store::matches::get(&db.pool, m1).await.unwrap().unwrap();
    let maps = store::matches::list_maps(&db.pool, m1).await.unwrap();
    assert_eq!(
        m.fields.scheduled_at,
        Some(at(18, 0)),
        "schedule: alpha wins"
    );
    assert_eq!(m.fields.lock_at, Some(at(18, 0)));
    assert_eq!(
        (maps[0].rounds_a, maps[0].rounds_b),
        (Some(13), Some(11)),
        "rounds: beta wins"
    );
    assert_eq!(
        maps[0].map_name.as_deref(),
        Some("Inferno"),
        "only alpha has names"
    );
    let prov = provenance(&db.pool, EntityType::Match, m1.0).await;
    assert_eq!(prov["schedule"], "alpha");
    assert_eq!(prov["map_rounds.1"], "beta");
    assert_eq!(prov["map_names.1"], "alpha");
    assert_eq!(count(&db.pool, "matches").await, 1);
    assert_eq!(count(&db.pool, "teams").await, 2);
    assert_eq!(
        store::events::list_stages(&db.pool, EventId(event))
            .await
            .unwrap()
            .len(),
        1,
        "beta's stage linked by name"
    );

    // Reversing the precedence reverses the result.
    let beta_first = reconciler(
        &db.pool,
        &[
            (FieldGroup::Schedule, &["beta", "alpha"][..]),
            (FieldGroup::MapRounds, &["alpha", "beta"][..]),
        ],
    );
    let mut rx = beta_first.bus().subscribe();
    beta_first.refresh_match(m1, now()).await.unwrap();
    let m = store::matches::get(&db.pool, m1).await.unwrap().unwrap();
    let maps = store::matches::list_maps(&db.pool, m1).await.unwrap();
    assert_eq!(m.fields.scheduled_at, Some(at(18, 30)));
    assert_eq!(m.fields.lock_at, Some(at(18, 30)));
    assert_eq!((maps[0].rounds_a, maps[0].rounds_b), (Some(13), Some(10)));
    let prov = provenance(&db.pool, EntityType::Match, m1.0).await;
    assert_eq!(
        (prov["schedule"].as_str(), prov["map_rounds.1"].as_str()),
        ("beta", "alpha")
    );
    assert_eq!(
        drain(&mut rx),
        vec![
            DomainEvent::MatchRescheduled {
                match_id: m1,
                from: Some(at(18, 0)),
                to: Some(at(18, 30)),
            },
            DomainEvent::MapUpdated {
                match_id: m1,
                number: 1
            },
        ]
    );
}

/// `manual` always comes first, whatever the sources say.
#[tokio::test]
async fn manual_beats_every_source() {
    let db = db().await;
    let r = reconciler(&db.pool, &[]);
    let (_, m1) = setup(&r, &db.pool).await;

    // What T05's manual service will do: a record plus a link.
    let manual = SrcMatch {
        scheduled_at: Some(at(21, 0)),
        status: Some(MatchStatus::Postponed),
        ..SrcMatch::new(m1.to_string(), "-")
    };
    let mut conn = db.pool.acquire().await.unwrap();
    record::store(
        &mut conn,
        MANUAL,
        EntityType::Match,
        &m1.to_string(),
        &manual,
        &serde_json::Value::Null,
        now(),
    )
    .await
    .unwrap();
    store::refs::link(
        &mut *conn,
        &store::refs::ExternalRef {
            entity_type: EntityType::Match,
            entity_id: m1.0,
            source: MANUAL.into(),
            external_id: m1.to_string(),
        },
    )
    .await
    .unwrap();
    drop(conn);

    r.refresh_match(m1, now()).await.unwrap();
    // A later sync of the sources does not undo it.
    r.ingest_matches("alpha", &[sourced(&alpha_m1())], now())
        .await
        .unwrap();
    let m = store::matches::get(&db.pool, m1).await.unwrap().unwrap();
    assert_eq!(m.fields.scheduled_at, Some(at(21, 0)));
    assert_eq!(m.fields.status, MatchStatus::Postponed);
    assert_eq!(
        m.fields.format,
        MatchFormat::Bo3,
        "manual has no format: alpha's"
    );
    let prov = provenance(&db.pool, EntityType::Match, m1.0).await;
    assert_eq!(
        (prov["schedule"].as_str(), prov["format"].as_str()),
        (MANUAL, "alpha")
    );
}

/// AC: a source with swapped team order is stored correctly flipped.
#[tokio::test]
async fn swapped_team_order_is_flipped() {
    let db = db().await;
    let r = reconciler(
        &db.pool,
        &[
            (FieldGroup::Series, &["beta"][..]),
            (FieldGroup::MapRounds, &["beta"][..]),
            (FieldGroup::Status, &["beta"][..]),
        ],
    );
    let (_, m1) = setup(&r, &db.pool).await;
    let vit = TeamId(
        resolve(&db.pool, "alpha", EntityType::Team, "a-vit")
            .await
            .unwrap(),
    );
    let navi = TeamId(
        resolve(&db.pool, "alpha", EntityType::Team, "a-navi")
            .await
            .unwrap(),
    );

    // Beta lists NAVI first: NAVI 1, Vitality 2; map 1 NAVI 11 : 13 Vitality.
    let swapped = SrcMatch {
        team_a: Some(b_navi()),
        team_b: Some(b_vit()),
        status: Some(MatchStatus::Finished),
        score: Some((1, 2)),
        winner: Some(Side::B),
        maps: vec![SrcMap {
            rounds: Some((11, 13)),
            winner: Some(Side::B),
            status: Some(MapStatus::Finished),
            ..SrcMap::new(1)
        }],
        ..beta_m1()
    };
    r.ingest_matches("beta", &[sourced(&swapped)], now())
        .await
        .unwrap();

    let m = store::matches::get(&db.pool, m1).await.unwrap().unwrap();
    assert_eq!(
        (m.fields.team_a_id, m.fields.team_b_id),
        (Some(vit), Some(navi)),
        "never swapped"
    );
    assert_eq!((m.fields.maps_won_a, m.fields.maps_won_b), (2, 1));
    assert_eq!(m.fields.winner_team_id, Some(vit));
    let maps = store::matches::list_maps(&db.pool, m1).await.unwrap();
    assert_eq!((maps[0].rounds_a, maps[0].rounds_b), (Some(13), Some(11)));
    assert_eq!(maps[0].winner_team_id, Some(vit));

    // The primary flipping its own order later changes nothing either.
    let before = snapshot(&db.pool).await;
    let alpha_swapped = SrcMatch {
        team_a: Some(a_navi()),
        team_b: Some(a_vit()),
        maps: vec![SrcMap {
            name: Some("Inferno".into()),
            rounds: Some((10, 13)),
            winner: Some(Side::B),
            ..SrcMap::new(1)
        }],
        ..alpha_m1()
    };
    let mut rx = r.bus().subscribe();
    r.ingest_matches("alpha", &[sourced(&alpha_swapped)], now())
        .await
        .unwrap();
    assert_eq!(snapshot(&db.pool).await, before);
    assert!(drain(&mut rx).is_empty());
}

/// AC: reconciling the same payload twice changes nothing and emits no
/// events.
#[tokio::test]
async fn same_payload_twice_changes_nothing() {
    let db = db().await;
    let r = reconciler(&db.pool, &[]);
    setup(&r, &db.pool).await;
    let batch_a = [sourced(&alpha_m1())];
    let batch_b = [sourced(&beta_m1())];
    r.ingest_matches("beta", &batch_b, now()).await.unwrap();

    let before = snapshot(&db.pool).await;
    let mut rx = r.bus().subscribe();
    let later = at(9, 30);
    let ra = r
        .ingest_event("alpha", &sourced(&alpha_event()), later)
        .await
        .unwrap();
    let rb = r
        .ingest_event("beta", &sourced(&beta_event()), later)
        .await
        .unwrap();
    let rc = r.ingest_matches("alpha", &batch_a, later).await.unwrap();
    let rd = r.ingest_matches("beta", &batch_b, later).await.unwrap();
    for rep in [ra, rb, rc, rd] {
        assert_eq!(
            (
                rep.created,
                rep.linked,
                rep.updated,
                rep.queued,
                rep.failed,
                rep.events
            ),
            (0, 0, 0, 0, 0, 0),
            "{rep:?}"
        );
    }
    assert_eq!(snapshot(&db.pool).await, before);
    assert!(drain(&mut rx).is_empty());
}

/// AC: unlinked entities show up in the queue and can be linked; linking
/// merges without duplicates.
#[tokio::test]
async fn unlinked_entities_queue_and_link_without_duplicates() {
    let db = db().await;
    let r = reconciler(&db.pool, &[]);
    let (_, m1) = setup(&r, &db.pool).await;
    let vit = resolve(&db.pool, "alpha", EntityType::Team, "a-vit")
        .await
        .unwrap();

    // Beta misspells Vitality: the team and its match wait in the queue.
    let typo = SrcMatch {
        team_a: Some(team("b-vit", "Vitaliti", None)),
        ..beta_m1()
    };
    let rep = r
        .ingest_matches("beta", &[sourced(&typo)], now())
        .await
        .unwrap();
    assert_eq!(rep.queued, 2, "{rep:?}");
    let queue = store::unlinked::list(&db.pool, Some("beta")).await.unwrap();
    let entries: Vec<_> = queue
        .iter()
        .map(|q| (q.entity_type, q.external_id.as_str(), q.reason))
        .collect();
    assert_eq!(
        entries,
        vec![
            (EntityType::Match, "b-m1", UnlinkedReason::Dependency),
            (EntityType::Team, "b-vit", UnlinkedReason::NoMatch),
        ]
    );
    assert_eq!(count(&db.pool, "teams").await, 2);
    assert_eq!(count(&db.pool, "matches").await, 1);

    // The admin links the team; the queued match resolves by itself.
    let rep = r
        .link(
            "beta",
            EntityType::Team,
            "b-vit",
            vit,
            "300000000000000001",
            now(),
        )
        .await
        .unwrap();
    assert_eq!(rep.linked, 2, "the team and then the match: {rep:?}");
    assert!(
        store::unlinked::list(&db.pool, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(count(&db.pool, "teams").await, 2, "no duplicate team");
    assert_eq!(count(&db.pool, "matches").await, 1, "no duplicate match");
    assert_eq!(
        resolve(&db.pool, "beta", EntityType::Match, "b-m1").await,
        Some(m1.0)
    );
    let canonical = store::teams::get(&db.pool, TeamId(vit))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(canonical.name, "Vitality", "alpha's name wins for the team");
    let audit = store::audit::list_for_entity(&db.pool, "team", &vit.to_string())
        .await
        .unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].record.action, "link_external_id");
    assert_eq!(audit[0].record.actor, "300000000000000001");

    // A team that does not exist yet: the admin creates it, then the match.
    let new_team = SrcMatch {
        team_a: Some(team("b-ast", "Astralis", None)),
        team_b: Some(b_navi()),
        scheduled_at: Some(at(12, 0)),
        ..SrcMatch::new("b-m2", "b-e1")
    };
    r.ingest_matches("beta", &[sourced(&new_team)], now())
        .await
        .unwrap();
    assert_eq!(
        store::unlinked::list(&db.pool, None).await.unwrap().len(),
        2
    );
    let err = r
        .create_from_unlinked("beta", EntityType::Match, "b-m2", "admin", now())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("link them first"), "{err}");
    let (ast, rep) = r
        .create_from_unlinked("beta", EntityType::Team, "b-ast", "admin", now())
        .await
        .unwrap();
    assert_eq!(rep.created, 1);
    let queue = store::unlinked::list(&db.pool, None).await.unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(
        queue[0].reason,
        UnlinkedReason::NoMatch,
        "no canonical match yet"
    );
    let (m2, _) = r
        .create_from_unlinked("beta", EntityType::Match, "b-m2", "admin", now())
        .await
        .unwrap();
    let m = store::matches::get(&db.pool, MatchId(m2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.fields.team_a_id, Some(TeamId(ast)));
    assert!(
        store::unlinked::list(&db.pool, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(count(&db.pool, "teams").await, 3);
    assert_eq!(count(&db.pool, "matches").await, 2);
}

// ---- identity rules, events, edge cases -------------------------------------------

#[tokio::test]
async fn identity_rules() {
    let db = db().await;
    let r = reconciler(&db.pool, &[]);
    let (_, m1) = setup(&r, &db.pool).await;
    // Alpha has two teams named like Liquid, and a second match at 12:00.
    let liquids = SrcMatch {
        team_a: Some(team("a-liq", "Liquid", None)),
        team_b: Some(team("a-tliq", "Team Liquid", None)),
        scheduled_at: Some(at(12, 0)),
        ..SrcMatch::new("a-m2", "a-e1")
    };
    r.ingest_matches("alpha", &[sourced(&liquids)], now())
        .await
        .unwrap();

    // `Team Vitality` → Vitality by name, `NaVi Esports` → NAVI by acronym;
    // 20:00 is exactly 2 h after 18:00 and still matches.
    let edge = SrcMatch {
        scheduled_at: Some(at(20, 0)),
        ..beta_m1()
    };
    // Names that match Vitality and NAVI, but beta links other IDs to them.
    let outside = SrcMatch {
        team_a: Some(team("b-x", "Vitality", None)),
        team_b: Some(team("b-y", "Natus Vincere", None)),
        scheduled_at: Some(at(20, 1)),
        ..SrcMatch::new("b-m9", "b-e1")
    };
    let ambiguous = SrcMatch {
        team_a: Some(team("b-liq", "Liquid", None)),
        team_b: Some(b_vit()),
        ..SrcMatch::new("b-m3", "b-e1")
    };
    r.ingest_matches("beta", &[sourced(&edge), sourced(&ambiguous)], now())
        .await
        .unwrap();
    assert_eq!(
        resolve(&db.pool, "beta", EntityType::Match, "b-m1").await,
        Some(m1.0)
    );
    assert_eq!(
        resolve(&db.pool, "beta", EntityType::Team, "b-navi").await,
        resolve(&db.pool, "alpha", EntityType::Team, "a-navi").await
    );
    let liq = store::unlinked::get(&db.pool, "beta", EntityType::Team, "b-liq")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(liq.reason, UnlinkedReason::Ambiguous);
    assert!(liq.detail.unwrap().contains("candidates"));

    // b-x matches Vitality by name, but beta already links b-vit to it.
    r.ingest_matches("beta", &[sourced(&outside)], now())
        .await
        .unwrap();
    assert_eq!(
        resolve(&db.pool, "beta", EntityType::Team, "b-x").await,
        None
    );
    let q = store::unlinked::get(&db.pool, "beta", EntityType::Team, "b-x")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(q.reason, UnlinkedReason::NoMatch);
    assert!(q.detail.unwrap().contains("already linked"));

    // A match 2 h 1 min away does not match.
    let late = SrcMatch {
        team_a: Some(team("b-liq2", "Team Liquid", None)),
        team_b: Some(team("b-liq1", "Liquid", None)),
        scheduled_at: Some(at(14, 1)),
        ..SrcMatch::new("b-m4", "b-e1")
    };
    r.ingest_matches("beta", &[sourced(&late)], now())
        .await
        .unwrap();
    // Both Liquids are ambiguous ("liquid" either way), so the match waits.
    let q = store::unlinked::get(&db.pool, "beta", EntityType::Match, "b-m4")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(q.reason, UnlinkedReason::Dependency);
    let liq = resolve(&db.pool, "alpha", EntityType::Team, "a-liq")
        .await
        .unwrap();
    let tliq = resolve(&db.pool, "alpha", EntityType::Team, "a-tliq")
        .await
        .unwrap();
    r.link("beta", EntityType::Team, "b-liq1", liq, "admin", now())
        .await
        .unwrap();
    // Linking one look-alike does not auto-link the other.
    let q = store::unlinked::get(&db.pool, "beta", EntityType::Team, "b-liq2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(q.reason, UnlinkedReason::Ambiguous);
    r.link("beta", EntityType::Team, "b-liq2", tliq, "admin", now())
        .await
        .unwrap();
    let q = store::unlinked::get(&db.pool, "beta", EntityType::Match, "b-m4")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        q.reason,
        UnlinkedReason::NoMatch,
        "14:01 is outside 12:00 ± 2 h"
    );
    let at_edge = SrcMatch {
        scheduled_at: Some(at(13, 59)),
        ..late
    };
    r.ingest_matches("beta", &[sourced(&at_edge)], now())
        .await
        .unwrap();
    assert_eq!(
        resolve(&db.pool, "beta", EntityType::Match, "b-m4").await,
        resolve(&db.pool, "alpha", EntityType::Match, "a-m2").await,
        "and the flipped team order still matches"
    );
}

#[tokio::test]
async fn lifecycle_emits_each_event_once() {
    let db = db().await;
    let r = reconciler(&db.pool, &[]);
    let mut rx = r.bus().subscribe();
    r.ingest_event("alpha", &sourced(&alpha_event()), now())
        .await
        .unwrap();
    let mut m = SrcMatch {
        maps: vec![],
        ..alpha_m1()
    };
    let mut step = async |m: &SrcMatch| {
        r.ingest_matches("alpha", &[sourced(m)], now())
            .await
            .unwrap();
        // Repeating a payload never emits anything.
        r.ingest_matches("alpha", &[sourced(m)], now())
            .await
            .unwrap();
        drain(&mut rx)
    };

    let created = step(&m).await;
    let id = created[0].match_id();
    assert_eq!(created, vec![DomainEvent::MatchCreated { match_id: id }]);

    m.scheduled_at = Some(at(19, 0));
    assert_eq!(
        step(&m).await,
        vec![DomainEvent::MatchRescheduled {
            match_id: id,
            from: Some(at(18, 0)),
            to: Some(at(19, 0))
        }]
    );

    m.status = Some(MatchStatus::Live);
    m.maps = vec![SrcMap {
        name: Some("Mirage".into()),
        status: Some(MapStatus::Live),
        ..SrcMap::new(1)
    }];
    assert_eq!(
        step(&m).await,
        vec![
            DomainEvent::MatchStarted { match_id: id },
            DomainEvent::MapUpdated {
                match_id: id,
                number: 1
            },
        ]
    );

    m.status = Some(MatchStatus::Finished);
    m.score = Some((1, 0));
    m.format = Some(MatchFormat::Bo1);
    m.maps[0].status = Some(MapStatus::Finished);
    m.maps[0].rounds = Some((13, 9));
    m.maps[0].winner = Some(Side::A);
    assert_eq!(
        step(&m).await,
        vec![
            DomainEvent::MatchFinished { match_id: id },
            DomainEvent::MapUpdated {
                match_id: id,
                number: 1
            },
        ]
    );
    let stored = store::matches::get(&db.pool, MatchId(id.0))
        .await
        .unwrap()
        .unwrap();
    assert!(
        stored.fields.winner_team_id.is_some(),
        "winner derived from 1:0"
    );

    m.maps[0].rounds = Some((16, 14));
    assert_eq!(
        step(&m).await,
        vec![
            DomainEvent::MapUpdated {
                match_id: id,
                number: 1
            },
            DomainEvent::ResultCorrected { match_id: id },
        ]
    );

    let mut other = SrcMatch {
        maps: vec![],
        scheduled_at: Some(at(22, 0)),
        ..alpha_m1()
    };
    other.ext_id = "a-m5".into();
    let created = step(&other).await;
    let other_id = created[0].match_id();
    other.status = Some(MatchStatus::Canceled);
    assert_eq!(
        step(&other).await,
        vec![DomainEvent::MatchCanceled { match_id: other_id }]
    );
}

#[tokio::test]
async fn tbd_slots_and_bracket_feeds() {
    let db = db().await;
    let r = reconciler(&db.pool, &[]);
    r.ingest_event("alpha", &sourced(&alpha_event()), now())
        .await
        .unwrap();
    // The final is listed before the semi it depends on.
    let fin = SrcMatch {
        stage_ext_id: Some("a-s1".into()),
        team_a: Some(a_vit()),
        format: Some(MatchFormat::Bo5),
        scheduled_at: Some(at(21, 0)),
        bracket: Some(SrcBracket {
            side: Some(domain::BracketSide::Final),
            round_index: Some(2),
            slot_index: Some(0),
            feeds_a: None,
            feeds_b: Some(SrcFeed {
                match_ext_id: "a-m1".into(),
                outcome: FeedOutcome::Winner,
            }),
        }),
        ..SrcMatch::new("a-fin", "a-e1")
    };
    r.ingest_matches("alpha", &[sourced(&fin), sourced(&alpha_m1())], now())
        .await
        .unwrap();
    let fin_id = MatchId(
        resolve(&db.pool, "alpha", EntityType::Match, "a-fin")
            .await
            .unwrap(),
    );
    let m1 = MatchId(
        resolve(&db.pool, "alpha", EntityType::Match, "a-m1")
            .await
            .unwrap(),
    );
    let vit = TeamId(
        resolve(&db.pool, "alpha", EntityType::Team, "a-vit")
            .await
            .unwrap(),
    );
    let navi = TeamId(
        resolve(&db.pool, "alpha", EntityType::Team, "a-navi")
            .await
            .unwrap(),
    );
    let f = store::matches::get(&db.pool, fin_id)
        .await
        .unwrap()
        .unwrap()
        .fields;
    assert_eq!(
        f.bracket.feeds_b_from.map(|x| x.match_id),
        Some(m1),
        "second pass"
    );
    assert_eq!(f.bracket.side, Some(domain::BracketSide::Final));
    assert!(f.stage_id.is_some());
    assert_eq!((f.team_a_id, f.team_b_id), (Some(vit), None));

    // The TBD slot is filled; alpha now lists the teams the other way round.
    let filled = SrcMatch {
        team_a: Some(a_navi()),
        team_b: Some(a_vit()),
        bracket: Some(SrcBracket {
            feeds_a: fin.bracket.as_ref().unwrap().feeds_b.clone(),
            feeds_b: None,
            ..fin.bracket.clone().unwrap()
        }),
        ..fin.clone()
    };
    r.ingest_matches("alpha", &[sourced(&filled)], now())
        .await
        .unwrap();
    let f = store::matches::get(&db.pool, fin_id)
        .await
        .unwrap()
        .unwrap()
        .fields;
    assert_eq!((f.team_a_id, f.team_b_id), (Some(vit), Some(navi)));
    assert_eq!(
        f.bracket.feeds_b_from.map(|x| x.match_id),
        Some(m1),
        "feed follows the flip"
    );
    assert_eq!(f.bracket.feeds_a_from, None);
}

#[tokio::test]
async fn replay_rebuilds_canonical_data_from_source_records() {
    let db = db().await;
    let r = reconciler(&db.pool, &[]);
    setup(&r, &db.pool).await;
    r.ingest_matches("beta", &[sourced(&beta_m1())], now())
        .await
        .unwrap();
    let mut done = alpha_m1();
    done.status = Some(MatchStatus::Finished);
    done.score = Some((2, 0));
    r.ingest_matches("alpha", &[sourced(&done)], now())
        .await
        .unwrap();

    // Replay on the same database is a no-op.
    let before = snapshot(&db.pool).await;
    let mut rx = r.bus().subscribe();
    let rep = r.replay(at(10, 0)).await.unwrap();
    assert_eq!(
        (rep.created, rep.updated, rep.failed, rep.events),
        (0, 0, 0, 0),
        "{rep:?}"
    );
    assert_eq!(snapshot(&db.pool).await, before);
    assert!(drain(&mut rx).is_empty());

    // A fresh database with only the raw records and the admin's event link
    // gets the same canonical data back, without any adapter. Internal IDs
    // may differ (creation order), so compare by natural keys.
    let natural = natural_snapshot(&db.pool).await;
    let fresh = db_with_records_of(&db.pool).await;
    store::refs::link(
        &fresh.pool,
        &store::refs::ExternalRef {
            entity_type: EntityType::Event,
            entity_id: 1,
            source: "beta".into(),
            external_id: "b-e1".into(),
        },
    )
    .await
    .unwrap();
    let r2 = reconciler(&fresh.pool, &[]);
    let rep = r2.replay(now()).await.unwrap();
    assert_eq!((rep.failed, rep.queued), (0, 0), "{rep:?}");
    assert_eq!(natural_snapshot(&fresh.pool).await, natural);
    let fresh_m1 = MatchId(
        resolve(&fresh.pool, "alpha", EntityType::Match, "a-m1")
            .await
            .unwrap(),
    );
    let m = store::matches::get(&fresh.pool, fresh_m1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (m.fields.status, m.fields.maps_won_a),
        (MatchStatus::Finished, 2)
    );
}

/// Canonical data keyed by names and external IDs instead of internal IDs.
async fn natural_snapshot(pool: &SqlitePool) -> Vec<String> {
    let team = |col: &str| format!("(SELECT name FROM teams WHERE id = {col})");
    let match_key = "JOIN external_refs r ON r.entity_type = 'match' AND r.entity_id = m.id";
    let queries = [
        "SELECT json_group_array(json_array(name, acronym, logo_url, country)) FROM (SELECT * FROM teams ORDER BY name)".to_owned(),
        "SELECT json_group_array(json_array(name, tier, begin_at, end_at, status)) FROM (SELECT * FROM events ORDER BY name)".to_owned(),
        "SELECT json_group_array(json_array(e.name, s.name, s.kind, s.position)) FROM stages s JOIN events e ON e.id = s.event_id".to_owned(),
        format!(
            "SELECT json_group_array(x) FROM (SELECT json_array(r.source, r.external_id, {}, {}, m.format, m.status, \
             m.scheduled_at, m.lock_at, m.maps_won_a, m.maps_won_b, {}, (SELECT name FROM stages WHERE id = m.stage_id)) AS x \
             FROM matches m {match_key} ORDER BY r.source, r.external_id)",
            team("m.team_a_id"),
            team("m.team_b_id"),
            team("m.winner_team_id"),
        ),
        format!(
            "SELECT json_group_array(x) FROM (SELECT json_array(r.source, r.external_id, p.number, p.map_name, p.status, \
             p.rounds_a, p.rounds_b, {}) AS x FROM maps p JOIN matches m ON m.id = p.match_id {match_key} \
             ORDER BY r.source, r.external_id, p.number)",
            team("p.winner_team_id"),
        ),
        format!(
            "SELECT json_group_array(x) FROM (SELECT json_array(r.source, r.external_id, f.field, f.source) AS x \
             FROM field_sources f JOIN matches m ON f.entity_type = 'match' AND f.entity_id = m.id {match_key} \
             ORDER BY r.source, r.external_id, f.field)"
        ),
        "SELECT json_group_array(json_array(source, entity_type, external_id)) FROM (SELECT * FROM external_refs ORDER BY source, entity_type, external_id)".to_owned(),
        "SELECT json_group_array(json_array(source, entity_type, external_id, reason)) FROM (SELECT * FROM unlinked_refs ORDER BY 1, 2, 3)".to_owned(),
    ];
    let mut out = Vec::new();
    for sql in queries {
        out.push(
            sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql))
                .fetch_one(pool)
                .await
                .unwrap(),
        );
    }
    out
}

async fn db_with_records_of(pool: &SqlitePool) -> Db {
    let fresh = db().await;
    for t in EntityType::ALL {
        for rec in store::refs::list_all_records(pool, *t).await.unwrap() {
            store::refs::upsert_record(&fresh.pool, &rec).await.unwrap();
        }
    }
    fresh
}

#[tokio::test]
async fn reconciler_futures_are_send() {
    let db = db().await;
    let r = Arc::new(reconciler(&db.pool, &[]));
    let r2 = Arc::clone(&r);
    tokio::spawn(async move {
        r2.ingest_event("alpha", &sourced(&alpha_event()), now())
            .await
            .unwrap();
        r2.ingest_matches("alpha", &[sourced(&alpha_m1())], now())
            .await
            .unwrap();
        r2.replay(now()).await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(count(&db.pool, "matches").await, 1);
}
