//! Repository round trips against a temp database.

mod common;

use common::{ALICE, BOB, GUILD, at, event, match_between, seed, team, test_db};
use domain::{
    BracketPos, BracketSide, EntityType, Event, EventStatus, EventTier, Feed, FeedOutcome, MapPick,
    MapStatus, Match, MatchFields, MatchFormat, MatchMap, MatchStatus, NewStage, PickParts,
    SeriesScore, Stage, StageKind, Team,
};
use serde_json::json;
use store::audit::AuditRecord;
use store::guilds::{FollowedEvent, GuildSettings};
use store::picks::PickResult;
use store::refs::{ExternalRef, RecordChange, SourceRecord};
use store::sessions::Session;
use store::sync_state::SyncState;
use store::users::{GuildMember, User};
use store::{Ts, audit, events, guilds, matches, picks, refs, sessions, sync_state, teams, users};

#[tokio::test]
async fn teams_round_trip() {
    let db = test_db().await;
    let id = team(&db.pool, "Bravo").await;
    let mut t = teams::get(&db.pool, id).await.unwrap().unwrap();
    assert_eq!(t.name, "Bravo");
    assert_eq!(t.acronym, None);

    t = Team {
        acronym: Some("BRV".into()),
        logo_url: Some("https://example.org/b.png".into()),
        country: Some("DE".into()),
        ..t
    };
    assert!(teams::update(&db.pool, &t).await.unwrap());
    assert_eq!(teams::get(&db.pool, id).await.unwrap(), Some(t));

    team(&db.pool, "Alpha").await;
    let names: Vec<_> = teams::list(&db.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["Alpha", "Bravo"]);
    assert!(
        teams::get(&db.pool, domain::TeamId(999))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn events_and_stages_round_trip() {
    let db = test_db().await;
    let id = event(&db.pool, "Major").await;
    let e = events::get(&db.pool, id).await.unwrap().unwrap();
    assert_eq!(e.tier, EventTier::S);
    assert_eq!(e.begin_at, Some(at(10, 0)));

    let e = Event {
        status: EventStatus::Running,
        end_at: Some(at(23, 59)),
        tier: EventTier::Unknown,
        ..e
    };
    assert!(events::update(&db.pool, &e).await.unwrap());
    assert_eq!(events::get(&db.pool, id).await.unwrap(), Some(e));

    let playoffs = events::insert_stage(
        &db.pool,
        &NewStage {
            event_id: id,
            name: "Playoffs".into(),
            kind: StageKind::DoubleElim,
            position: 2,
        },
    )
    .await
    .unwrap();
    let swiss = events::insert_stage(
        &db.pool,
        &NewStage {
            event_id: id,
            name: "Swiss".into(),
            kind: StageKind::Swiss,
            position: 1,
        },
    )
    .await
    .unwrap();
    let stages = events::list_stages(&db.pool, id).await.unwrap();
    assert_eq!(
        stages.iter().map(|s| s.id).collect::<Vec<_>>(),
        [swiss, playoffs]
    );

    let renamed = Stage {
        name: "Playoffs (LAN)".into(),
        ..stages[1].clone()
    };
    assert!(events::update_stage(&db.pool, &renamed).await.unwrap());
    assert_eq!(
        events::get_stage(&db.pool, playoffs).await.unwrap(),
        Some(renamed)
    );
}

#[tokio::test]
async fn matches_round_trip_every_field() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let stage = events::insert_stage(
        &db.pool,
        &NewStage {
            event_id: s.event,
            name: "Playoffs".into(),
            kind: StageKind::DoubleElim,
            position: 1,
        },
    )
    .await
    .unwrap();

    // A TBD grand final fed by the winners of two earlier matches.
    let semi = match_between(
        &db.pool,
        s.event,
        Some(s.team_a),
        Some(s.team_c),
        MatchFormat::Bo3,
    )
    .await;
    let fields = MatchFields {
        stage_id: Some(stage),
        bracket: BracketPos {
            side: Some(BracketSide::Final),
            round_index: Some(3),
            slot_index: Some(0),
            feeds_a_from: Some(Feed {
                match_id: s.bo3,
                outcome: FeedOutcome::Winner,
            }),
            feeds_b_from: Some(Feed {
                match_id: semi,
                outcome: FeedOutcome::Loser,
            }),
        },
        ..MatchFields::scheduled(s.event, None, None, MatchFormat::Bo5, Some(at(20, 0)))
    };
    let id = matches::insert(&db.pool, &fields).await.unwrap();
    let stored = matches::get(&db.pool, id).await.unwrap().unwrap();
    assert_eq!(stored, Match { id, fields });

    // Teams arrive, the match is played and finished.
    let mut m = stored;
    m.fields.team_a_id = Some(s.team_a);
    m.fields.team_b_id = Some(s.team_b);
    m.fields.status = MatchStatus::Finished;
    m.fields.started_at = Some(at(20, 5));
    m.fields.ended_at = Some(at(23, 0));
    m.fields.locked = true;
    m.fields.voided = true;
    m.fields.maps_won_a = 3;
    m.fields.maps_won_b = 1;
    m.fields.winner_team_id = Some(s.team_a);
    assert!(matches::update(&db.pool, &m).await.unwrap());
    assert_eq!(matches::get(&db.pool, id).await.unwrap(), Some(m));
}

#[tokio::test]
async fn matches_of_an_event_are_ordered_by_time() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let unscheduled = matches::insert(
        &db.pool,
        &MatchFields::scheduled(s.event, None, None, MatchFormat::Bo1, None),
    )
    .await
    .unwrap();
    let early = matches::insert(
        &db.pool,
        &MatchFields::scheduled(s.event, None, None, MatchFormat::Bo1, Some(at(9, 30))),
    )
    .await
    .unwrap();
    let other_event = event(&db.pool, "Other").await;
    match_between(&db.pool, other_event, None, None, MatchFormat::Bo1).await;

    let ids: Vec<_> = matches::list_for_event(&db.pool, s.event)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, [early, s.bo3, unscheduled]);
}

#[tokio::test]
async fn stored_times_compare_as_strings() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    // A lock_at with milliseconds, compared in SQL against a bound time as
    // the lock ticker (T06) will do.
    let t = at(9, 59) + chrono::Duration::milliseconds(59_500);
    let early = matches::insert(
        &db.pool,
        &MatchFields::scheduled(s.event, None, None, MatchFormat::Bo1, Some(t)),
    )
    .await
    .unwrap();
    let due: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM matches WHERE locked = 0 AND lock_at <= ? ORDER BY lock_at",
    )
    .bind(Ts(at(10, 0)))
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(due, [early.0]);
    let m = matches::get(&db.pool, early).await.unwrap().unwrap();
    assert_eq!(m.fields.lock_at, Some(t));
}

#[tokio::test]
async fn maps_upsert_by_number() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let mut map2 = MatchMap {
        match_id: s.bo3,
        number: 2,
        map_name: None,
        status: MapStatus::Upcoming,
        rounds_a: None,
        rounds_b: None,
        winner_team_id: None,
    };
    let map1 = MatchMap {
        number: 1,
        map_name: Some("Inferno".into()),
        status: MapStatus::Finished,
        rounds_a: Some(16),
        rounds_b: Some(14),
        winner_team_id: Some(s.team_a),
        ..map2.clone()
    };
    matches::upsert_map(&db.pool, &map2).await.unwrap();
    matches::upsert_map(&db.pool, &map1).await.unwrap();
    map2.map_name = Some("Mirage".into());
    map2.status = MapStatus::Live;
    map2.rounds_a = Some(3);
    map2.rounds_b = Some(5);
    matches::upsert_map(&db.pool, &map2).await.unwrap();

    assert_eq!(
        matches::list_maps(&db.pool, s.bo3).await.unwrap(),
        [map1, map2]
    );
}

#[tokio::test]
async fn external_refs_resolve_and_relink() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let r = ExternalRef {
        entity_type: EntityType::Team,
        entity_id: s.team_a.0,
        source: "pandascore".into(),
        external_id: "3212".into(),
    };
    refs::link(&db.pool, &r).await.unwrap();
    let resolve = || refs::resolve(&db.pool, "pandascore", EntityType::Team, "3212");
    assert_eq!(resolve().await.unwrap(), Some(s.team_a.0));
    assert_eq!(
        refs::resolve(&db.pool, "pandascore", EntityType::Match, "3212")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        refs::refs_of(&db.pool, EntityType::Team, s.team_a.0)
            .await
            .unwrap(),
        std::slice::from_ref(&r)
    );

    // Merging duplicates moves the link.
    refs::relink(
        &db.pool,
        &ExternalRef {
            entity_id: s.team_b.0,
            ..r
        },
    )
    .await
    .unwrap();
    assert_eq!(resolve().await.unwrap(), Some(s.team_b.0));
    assert!(
        refs::refs_of(&db.pool, EntityType::Team, s.team_a.0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn source_records_keep_the_latest_payload() {
    let db = test_db().await;
    let rec = SourceRecord {
        source: "pandascore".into(),
        entity_type: EntityType::Match,
        external_id: "1001".into(),
        payload: r#"{"status":"not_started"}"#.into(),
        payload_hash: "h1".into(),
        fetched_at: at(12, 0),
    };
    let get = || refs::get_record(&db.pool, "pandascore", EntityType::Match, "1001");

    assert_eq!(
        refs::upsert_record(&db.pool, &rec).await.unwrap(),
        RecordChange::Inserted
    );

    // Same hash: only fetched_at moves.
    let refetch = SourceRecord {
        fetched_at: at(12, 30),
        ..rec.clone()
    };
    assert_eq!(
        refs::upsert_record(&db.pool, &refetch).await.unwrap(),
        RecordChange::Unchanged
    );
    assert_eq!(get().await.unwrap(), Some(refetch));

    let changed = SourceRecord {
        payload: r#"{"status":"running"}"#.into(),
        payload_hash: "h2".into(),
        fetched_at: at(13, 0),
        ..rec
    };
    assert_eq!(
        refs::upsert_record(&db.pool, &changed).await.unwrap(),
        RecordChange::Changed
    );
    assert_eq!(get().await.unwrap(), Some(changed.clone()));
    assert_eq!(
        refs::list_records(&db.pool, "pandascore", EntityType::Match)
            .await
            .unwrap(),
        [changed]
    );
}

#[tokio::test]
async fn users_and_members_upsert() {
    let db = test_db().await;
    let mut u = User {
        discord_user_id: ALICE.into(),
        username: "alice".into(),
        global_name: None,
        avatar_hash: None,
        updated_at: at(9, 0),
    };
    users::upsert_user(&db.pool, &u).await.unwrap();
    u.global_name = Some("Alice".into());
    u.avatar_hash = Some("a1b2".into());
    u.updated_at = at(10, 0);
    users::upsert_user(&db.pool, &u).await.unwrap();
    assert_eq!(users::get_user(&db.pool, ALICE).await.unwrap(), Some(u));

    let mut m = GuildMember {
        guild_id: GUILD.into(),
        user_id: ALICE.into(),
        nick: Some("ali".into()),
        last_seen_at: at(10, 0),
    };
    users::upsert_member(&db.pool, &m).await.unwrap();
    m.nick = None;
    m.last_seen_at = at(11, 0);
    users::upsert_member(&db.pool, &m).await.unwrap();
    assert_eq!(
        users::get_member(&db.pool, GUILD, ALICE).await.unwrap(),
        Some(m)
    );
}

#[tokio::test]
async fn saving_a_pick_twice_edits_it_and_keeps_history() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let first = PickParts {
        winner_team_id: Some(s.team_a),
        series: Some(SeriesScore { a: 2, b: 0 }),
        maps: vec![
            MapPick {
                map_number: 2,
                winner_team_id: s.team_a,
                rounds: Some((13, 9)),
                overtime: false,
            },
            MapPick {
                map_number: 1,
                winner_team_id: s.team_a,
                rounds: None,
                overtime: false,
            },
        ],
    };
    let p1 = picks::save(&db.pool, GUILD, ALICE, s.bo3, &first, at(12, 0))
        .await
        .unwrap();
    // Maps come back sorted by number.
    assert_eq!(
        p1.parts
            .maps
            .iter()
            .map(|m| m.map_number)
            .collect::<Vec<_>>(),
        [1, 2]
    );

    let second = PickParts {
        winner_team_id: Some(s.team_b),
        series: None,
        maps: vec![MapPick {
            map_number: 3,
            winner_team_id: s.team_b,
            rounds: None,
            overtime: true,
        }],
    };
    let p2 = picks::save(&db.pool, GUILD, ALICE, s.bo3, &second, at(12, 30))
        .await
        .unwrap();
    assert_eq!(p2.id, p1.id);
    assert_eq!(p2.created_at, at(12, 0));
    assert_eq!(p2.updated_at, at(12, 30));
    assert_eq!(p2.parts, second);
    assert_eq!(
        picks::get(&db.pool, GUILD, ALICE, s.bo3).await.unwrap(),
        Some(p2.clone())
    );

    let revisions = picks::list_revisions(&db.pool, p1.id).await.unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].edited_at, at(12, 0));
    assert_eq!(revisions[0].snapshot, picks::snapshot(&first));
    assert_eq!(revisions[0].snapshot["series"], json!({ "a": 2, "b": 0 }));
    assert_eq!(revisions[1].snapshot, picks::snapshot(&second));
    assert_eq!(revisions[1].snapshot["maps"][0]["overtime"], json!(true));
}

#[tokio::test]
async fn picks_of_a_match_include_every_member() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let map = |n, t| MapPick {
        map_number: n,
        winner_team_id: t,
        rounds: None,
        overtime: false,
    };
    let alice = PickParts {
        maps: vec![map(1, s.team_a), map(2, s.team_b)],
        ..PickParts::default()
    };
    let bob = PickParts {
        winner_team_id: Some(s.team_b),
        ..PickParts::default()
    };
    picks::save(&db.pool, GUILD, ALICE, s.bo3, &alice, at(12, 0))
        .await
        .unwrap();
    picks::save(&db.pool, GUILD, BOB, s.bo3, &bob, at(12, 1))
        .await
        .unwrap();
    // Another guild's pick is not listed.
    picks::save(&db.pool, "100000000000000002", BOB, s.bo3, &bob, at(12, 2))
        .await
        .unwrap();

    let all = picks::list_for_match(&db.pool, GUILD, s.bo3).await.unwrap();
    assert_eq!(
        all.iter()
            .map(|p| (p.user_id.as_str(), &p.parts))
            .collect::<Vec<_>>(),
        [(ALICE, &alice), (BOB, &bob)]
    );
}

#[tokio::test]
async fn deleting_a_pick_removes_its_parts_history_and_result() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let parts = PickParts {
        winner_team_id: Some(s.team_a),
        maps: vec![MapPick {
            map_number: 1,
            winner_team_id: s.team_a,
            rounds: Some((13, 4)),
            overtime: false,
        }],
        ..PickParts::default()
    };
    let p = picks::save(&db.pool, GUILD, ALICE, s.bo3, &parts, at(12, 0))
        .await
        .unwrap();
    picks::upsert_result(
        &db.pool,
        &PickResult {
            pick_id: p.id,
            points: 3,
            breakdown: json!({ "winner": 3 }),
            rules_version: 1,
            settled_at: at(22, 0),
        },
    )
    .await
    .unwrap();

    assert!(picks::delete(&db.pool, GUILD, ALICE, s.bo3).await.unwrap());
    assert!(!picks::delete(&db.pool, GUILD, ALICE, s.bo3).await.unwrap());
    assert!(
        picks::get(&db.pool, GUILD, ALICE, s.bo3)
            .await
            .unwrap()
            .is_none()
    );
    for table in ["pick_maps", "pick_revisions", "pick_results"] {
        let n: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(n, 0, "{table} not emptied");
    }
}

#[tokio::test]
async fn pick_results_are_overwritten_not_added() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let p = picks::save(
        &db.pool,
        GUILD,
        ALICE,
        s.bo3,
        &PickParts {
            winner_team_id: Some(s.team_a),
            ..PickParts::default()
        },
        at(12, 0),
    )
    .await
    .unwrap();
    let mut r = PickResult {
        pick_id: p.id,
        points: 3,
        breakdown: json!({ "winner": 3 }),
        rules_version: 1,
        settled_at: at(22, 0),
    };
    picks::upsert_result(&db.pool, &r).await.unwrap();
    // A corrected result recomputes the pick.
    r.points = 0;
    r.breakdown = json!({ "winner": 0 });
    r.settled_at = at(23, 0);
    picks::upsert_result(&db.pool, &r).await.unwrap();
    picks::upsert_result(&db.pool, &r).await.unwrap();
    assert_eq!(picks::get_result(&db.pool, p.id).await.unwrap(), Some(r));
}

#[tokio::test]
async fn guild_settings_and_followed_events() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    assert!(
        guilds::get_settings(&db.pool, GUILD)
            .await
            .unwrap()
            .is_none()
    );

    let mut settings = GuildSettings {
        guild_id: GUILD.into(),
        announce_channel_id: Some("300000000000000001".into()),
        admin_role_id: None,
        scoring_rules: None,
        scoring_rules_version: 1,
        updated_at: at(8, 0),
    };
    guilds::upsert_settings(&db.pool, &settings).await.unwrap();
    settings.admin_role_id = Some("400000000000000001".into());
    settings.scoring_rules = Some(json!({ "winner": 5 }));
    settings.scoring_rules_version = 2;
    settings.updated_at = at(9, 0);
    guilds::upsert_settings(&db.pool, &settings).await.unwrap();
    assert_eq!(
        guilds::get_settings(&db.pool, GUILD).await.unwrap(),
        Some(settings)
    );

    let other = event(&db.pool, "Other").await;
    let follow = |event_id, followed_at| FollowedEvent {
        guild_id: GUILD.into(),
        event_id,
        followed_by: ALICE.into(),
        followed_at,
    };
    assert!(
        guilds::follow_event(&db.pool, &follow(other, at(9, 0)))
            .await
            .unwrap()
    );
    assert!(
        guilds::follow_event(&db.pool, &follow(s.event, at(10, 0)))
            .await
            .unwrap()
    );
    // Following again keeps the original follow.
    assert!(
        !guilds::follow_event(&db.pool, &follow(s.event, at(11, 0)))
            .await
            .unwrap()
    );
    assert_eq!(
        guilds::list_followed(&db.pool, GUILD).await.unwrap(),
        [follow(other, at(9, 0)), follow(s.event, at(10, 0))]
    );
    assert!(guilds::is_followed(&db.pool, GUILD, s.event).await.unwrap());

    assert!(
        guilds::unfollow_event(&db.pool, GUILD, s.event)
            .await
            .unwrap()
    );
    assert!(
        !guilds::unfollow_event(&db.pool, GUILD, s.event)
            .await
            .unwrap()
    );
    assert!(!guilds::is_followed(&db.pool, GUILD, s.event).await.unwrap());
}

#[tokio::test]
async fn audit_log_appends_in_order() {
    let db = test_db().await;
    let record = |action: &str, after| AuditRecord {
        actor: ALICE.into(),
        action: action.into(),
        entity_type: "match".into(),
        entity_id: Some("7".into()),
        before: None,
        after,
        at: at(15, 0),
    };
    let first = record("void", None);
    let second = AuditRecord {
        before: Some(json!({ "status": "finished" })),
        ..record("set_status", Some(json!({ "status": "canceled" })))
    };
    let id1 = audit::append(&db.pool, &first).await.unwrap();
    let id2 = audit::append(&db.pool, &second).await.unwrap();
    audit::append(
        &db.pool,
        &AuditRecord {
            entity_id: Some("8".into()),
            ..first.clone()
        },
    )
    .await
    .unwrap();

    let entries = audit::list_for_entity(&db.pool, "match", "7")
        .await
        .unwrap();
    assert_eq!(
        entries
            .into_iter()
            .map(|e| (e.id, e.record))
            .collect::<Vec<_>>(),
        [(id1, first), (id2, second)]
    );
}

#[tokio::test]
async fn sync_state_round_trip() {
    let db = test_db().await;
    assert!(
        sync_state::get(&db.pool, "live", "pandascore")
            .await
            .unwrap()
            .is_none()
    );

    let mut st = SyncState::new("live", "pandascore", at(12, 0));
    sync_state::put(&db.pool, &st).await.unwrap();
    st.cursor = Some("page=2".into());
    st.last_run_at = Some(at(12, 2));
    st.last_success_at = Some(at(12, 2));
    st.last_error = Some("timeout".into());
    st.request_count = 17;
    st.window_started_at = Some(at(12, 0));
    st.remaining_quota = Some(983);
    st.updated_at = at(12, 2);
    sync_state::put(&db.pool, &st).await.unwrap();
    assert_eq!(
        sync_state::get(&db.pool, "live", "pandascore")
            .await
            .unwrap(),
        Some(st.clone())
    );

    sync_state::put(
        &db.pool,
        &SyncState::new("imminent", "pandascore", at(12, 3)),
    )
    .await
    .unwrap();
    let jobs: Vec<_> = sync_state::list(&db.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.job)
        .collect();
    assert_eq!(jobs, ["imminent", "live"]);
}

#[tokio::test]
async fn repositories_compose_in_one_transaction() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let mut tx = db.pool.begin().await.unwrap();
    let t = team(&db.pool, "Outside").await;
    let inside = teams::insert(
        &mut *tx,
        &domain::NewTeam {
            name: "Inside".into(),
            acronym: None,
            logo_url: None,
            country: None,
        },
    )
    .await
    .unwrap();
    picks::save(
        &mut *tx,
        GUILD,
        ALICE,
        s.bo3,
        &PickParts {
            winner_team_id: Some(s.team_a),
            ..PickParts::default()
        },
        at(12, 0),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    assert!(teams::get(&db.pool, t).await.unwrap().is_some());
    assert!(teams::get(&db.pool, inside).await.unwrap().is_none());
    assert!(
        picks::get(&db.pool, GUILD, ALICE, s.bo3)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn lock_due_locks_started_and_passed_matches_once() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    // `bo3` locks at 18:00. A second match locks at 20:00 but is live.
    let live = match_between(
        &db.pool,
        s.event,
        Some(s.team_a),
        Some(s.team_c),
        MatchFormat::Bo1,
    )
    .await;
    let mut m = matches::get(&db.pool, live).await.unwrap().unwrap();
    m.fields.status = MatchStatus::Live;
    m.fields.lock_at = Some(at(20, 0));
    matches::update(&db.pool, &m).await.unwrap();
    // No schedule: never locked by time.
    let unscheduled = matches::insert(
        &db.pool,
        &MatchFields::scheduled(s.event, None, None, MatchFormat::Bo3, None),
    )
    .await
    .unwrap();

    assert_eq!(
        matches::lock_due(&db.pool, at(17, 59)).await.unwrap(),
        vec![live]
    );
    assert_eq!(
        matches::lock_due(&db.pool, at(18, 0)).await.unwrap(),
        vec![s.bo3]
    );
    assert!(
        matches::lock_due(&db.pool, at(23, 0))
            .await
            .unwrap()
            .is_empty()
    );
    let get = |id| matches::get(&db.pool, id);
    assert!(get(s.bo3).await.unwrap().unwrap().fields.locked);
    assert!(!get(unscheduled).await.unwrap().unwrap().fields.locked);
}

#[tokio::test]
async fn followed_event_ids_span_guilds() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let other = event(&db.pool, "Other").await;
    assert!(
        guilds::followed_event_ids(&db.pool)
            .await
            .unwrap()
            .is_empty()
    );
    for (guild, ev) in [
        (GUILD, other),
        (GUILD, s.event),
        ("100000000000000002", s.event),
    ] {
        guilds::follow_event(
            &db.pool,
            &FollowedEvent {
                guild_id: guild.into(),
                event_id: ev,
                followed_by: ALICE.into(),
                followed_at: at(9, 0),
            },
        )
        .await
        .unwrap();
    }
    assert_eq!(
        guilds::followed_event_ids(&db.pool).await.unwrap(),
        vec![s.event, other]
    );
}

#[tokio::test]
async fn match_refs_for_event_lists_one_source() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let other_event = event(&db.pool, "Other").await;
    let elsewhere = match_between(&db.pool, other_event, None, None, MatchFormat::Bo1).await;
    for (id, source, ext) in [
        (s.bo3.0, "pandascore", "77"),
        (s.bo3.0, "other", "x"),
        (elsewhere.0, "pandascore", "78"),
    ] {
        refs::link(
            &db.pool,
            &ExternalRef {
                entity_type: EntityType::Match,
                entity_id: id,
                source: source.into(),
                external_id: ext.into(),
            },
        )
        .await
        .unwrap();
    }
    assert_eq!(
        refs::match_refs_for_event(&db.pool, "pandascore", s.event.0)
            .await
            .unwrap(),
        vec![(s.bo3.0, "77".to_owned())]
    );
}

#[tokio::test]
async fn sessions_expire_and_follow_their_user() {
    let db = test_db().await;
    common::user(&db.pool, ALICE).await;
    common::user(&db.pool, BOB).await;
    let session = |hash: char, user: &str, start: u32| Session {
        token_hash: hash.to_string().repeat(64),
        user_id: user.into(),
        guild_id: GUILD.into(),
        created_at: at(start, 0),
        expires_at: at(start + 2, 0),
    };
    let a = session('a', ALICE, 10);
    let b = session('b', ALICE, 12);
    let c = session('c', BOB, 10);
    for s in [&a, &b, &c] {
        sessions::create(&db.pool, s).await.unwrap();
    }

    // Valid strictly before `expires_at`.
    let hash = a.token_hash.as_str();
    assert_eq!(
        sessions::get_valid(&db.pool, hash, at(11, 59))
            .await
            .unwrap(),
        Some(a.clone())
    );
    assert_eq!(
        sessions::get_valid(&db.pool, hash, at(12, 0))
            .await
            .unwrap(),
        None
    );
    let unknown = "f".repeat(64);
    assert_eq!(
        sessions::get_valid(&db.pool, &unknown, at(10, 0))
            .await
            .unwrap(),
        None
    );

    // At 12:00, `a` and `c` have expired; `b` stays.
    assert_eq!(
        sessions::delete_expired(&db.pool, at(12, 0)).await.unwrap(),
        2
    );
    assert_eq!(
        sessions::get_valid(&db.pool, &b.token_hash, at(12, 0))
            .await
            .unwrap(),
        Some(b.clone())
    );

    // Deleting a user deletes their sessions.
    sqlx::query("DELETE FROM users WHERE discord_user_id = ?")
        .bind(ALICE)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(!sessions::delete(&db.pool, &b.token_hash).await.unwrap());

    sessions::create(&db.pool, &c).await.unwrap();
    assert!(sessions::delete(&db.pool, &c.token_hash).await.unwrap());
}
