//! The schema enforces its invariants on its own, also for writes that
//! bypass the repositories.

mod common;

use common::{ALICE, BOB, GUILD, assert_rejected, at, exec, match_between, seed, test_db};
use domain::{
    BracketSide, EntityType, EventStatus, EventTier, FeedOutcome, MapPick, MapStatus, MatchFormat,
    MatchStatus, PickParts, SeriesScore, StageKind,
};
use store::refs::{self, ExternalRef};

fn winner_only(team: domain::TeamId) -> PickParts {
    PickParts {
        winner_team_id: Some(team),
        ..PickParts::default()
    }
}

#[tokio::test]
async fn one_pick_per_user_and_match() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    store::picks::save(
        &db.pool,
        GUILD,
        ALICE,
        s.bo3,
        &winner_only(s.team_a),
        at(12, 0),
    )
    .await
    .unwrap();

    assert_rejected(
        exec(
            &db.pool,
            format!(
                "INSERT INTO picks (guild_id, user_id, match_id, created_at, updated_at) \
                 VALUES ('{GUILD}', '{ALICE}', {}, '2026-11-20T12:00:00.000Z', \
                 '2026-11-20T12:00:00.000Z')",
                s.bo3
            ),
        )
        .await,
        "UNIQUE constraint failed: picks.guild_id, picks.user_id, picks.match_id",
    );

    // Another user, or the same user in another guild, may pick the match.
    store::picks::save(
        &db.pool,
        GUILD,
        BOB,
        s.bo3,
        &winner_only(s.team_b),
        at(12, 0),
    )
    .await
    .unwrap();
    store::picks::save(
        &db.pool,
        "100000000000000002",
        ALICE,
        s.bo3,
        &winner_only(s.team_b),
        at(12, 0),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn every_domain_enum_value_is_accepted() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    for tier in EventTier::ALL {
        for status in EventStatus::ALL {
            exec(
                &db.pool,
                format!(
                    "INSERT INTO events (name, tier, status) VALUES ('e', '{tier}', '{status}')"
                ),
            )
            .await
            .unwrap();
        }
    }
    for kind in StageKind::ALL {
        exec(
            &db.pool,
            format!(
                "INSERT INTO stages (event_id, name, kind) VALUES ({}, 's', '{kind}')",
                s.event
            ),
        )
        .await
        .unwrap();
    }
    for format in MatchFormat::ALL {
        for status in MatchStatus::ALL {
            for side in BracketSide::ALL {
                for outcome in FeedOutcome::ALL {
                    exec(
                        &db.pool,
                        format!(
                            "INSERT INTO matches (event_id, format, status, bracket_side, \
                               feeds_a_from_match_id, feeds_a_from_outcome) \
                             VALUES ({}, '{format}', '{status}', '{side}', {}, '{outcome}')",
                            s.event, s.bo3
                        ),
                    )
                    .await
                    .unwrap();
                }
            }
        }
    }
    let bo5 = match_between(
        &db.pool,
        s.event,
        Some(s.team_a),
        Some(s.team_b),
        MatchFormat::Bo5,
    )
    .await;
    for (number, status) in (1..).zip(MapStatus::ALL) {
        exec(
            &db.pool,
            format!(
                "INSERT INTO maps (match_id, number, status) VALUES ({bo5}, {number}, '{status}')"
            ),
        )
        .await
        .unwrap();
    }
    for (i, entity_type) in EntityType::ALL.iter().enumerate() {
        refs::link(
            &db.pool,
            &ExternalRef {
                entity_type: *entity_type,
                entity_id: 1,
                source: "fixture".into(),
                external_id: i.to_string(),
            },
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn invalid_enum_values_are_rejected() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let (e, m) = (s.event, s.bo3);
    let cases = [
        "INSERT INTO events (name, tier, status) VALUES ('e', 'x', 'upcoming')".to_owned(),
        "INSERT INTO events (name, status) VALUES ('e', 'live')".to_owned(),
        format!("INSERT INTO stages (event_id, name, kind) VALUES ({e}, 's', 'round_robin')"),
        format!("INSERT INTO matches (event_id, format) VALUES ({e}, 'bo7')"),
        format!("INSERT INTO matches (event_id, format) VALUES ({e}, 'BO3')"),
        format!("INSERT INTO matches (event_id, format, status) VALUES ({e}, 'bo3', 'done')"),
        format!("INSERT INTO matches (event_id, format, locked) VALUES ({e}, 'bo3', 2)"),
        format!(
            "INSERT INTO matches (event_id, format, bracket_side) VALUES ({e}, 'bo3', 'middle')"
        ),
        format!(
            "INSERT INTO matches (event_id, format, feeds_a_from_match_id, feeds_a_from_outcome) \
             VALUES ({e}, 'bo3', {m}, 'draw')"
        ),
        format!("INSERT INTO maps (match_id, number, status) VALUES ({m}, 1, 'played')"),
        "INSERT INTO external_refs (entity_type, entity_id, source, external_id) \
         VALUES ('player', 1, 'fixture', '1')"
            .to_owned(),
    ];
    for sql in cases {
        let res = exec(&db.pool, sql.clone()).await;
        assert!(
            matches!(&res, Err(sqlx::Error::Database(d)) if d.message().contains("CHECK constraint failed")),
            "{sql}\n=> {res:?}"
        );
    }
}

#[tokio::test]
async fn map_number_must_fit_the_format() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    for format in MatchFormat::ALL {
        let m = match_between(&db.pool, s.event, Some(s.team_a), Some(s.team_b), *format).await;
        let max = format.max_maps();
        for number in 1..=max {
            exec(
                &db.pool,
                format!("INSERT INTO maps (match_id, number) VALUES ({m}, {number})"),
            )
            .await
            .unwrap();
        }
        assert_rejected(
            exec(
                &db.pool,
                format!(
                    "INSERT INTO maps (match_id, number) VALUES ({m}, {})",
                    max + 1
                ),
            )
            .await,
            "map number exceeds match format",
        );
        assert_rejected(
            exec(
                &db.pool,
                format!("INSERT INTO maps (match_id, number) VALUES ({m}, 0)"),
            )
            .await,
            "CHECK constraint failed",
        );

        // Map picks follow the same rule.
        let maps: Vec<MapPick> = (1..=max + 1)
            .map(|n| MapPick {
                map_number: n,
                winner_team_id: s.team_a,
                rounds: None,
                overtime: false,
            })
            .collect();
        let ok = PickParts {
            maps: maps[..usize::from(max)].to_vec(),
            ..PickParts::default()
        };
        store::picks::save(&db.pool, GUILD, ALICE, m, &ok, at(12, 0))
            .await
            .unwrap();
        let too_many = PickParts {
            maps,
            ..PickParts::default()
        };
        assert_rejected(
            store::picks::save(&db.pool, GUILD, BOB, m, &too_many, at(12, 0)).await,
            "map number exceeds match format",
        );
        // The failed save rolled back completely.
        assert!(
            store::picks::get(&db.pool, GUILD, BOB, m)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn format_cannot_shrink_below_existing_maps() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    exec(
        &db.pool,
        format!("INSERT INTO maps (match_id, number) VALUES ({}, 3)", s.bo3),
    )
    .await
    .unwrap();
    assert_rejected(
        exec(
            &db.pool,
            format!("UPDATE matches SET format = 'bo1' WHERE id = {}", s.bo3),
        )
        .await,
        "match format is shorter than its existing maps",
    );
    exec(
        &db.pool,
        format!("UPDATE matches SET format = 'bo5' WHERE id = {}", s.bo3),
    )
    .await
    .unwrap();

    // Map picks count too.
    let m = match_between(
        &db.pool,
        s.event,
        Some(s.team_a),
        Some(s.team_b),
        MatchFormat::Bo3,
    )
    .await;
    let parts = PickParts {
        maps: vec![MapPick {
            map_number: 2,
            winner_team_id: s.team_a,
            rounds: None,
            overtime: false,
        }],
        ..PickParts::default()
    };
    store::picks::save(&db.pool, GUILD, ALICE, m, &parts, at(12, 0))
        .await
        .unwrap();
    assert_rejected(
        exec(
            &db.pool,
            format!("UPDATE matches SET format = 'bo1' WHERE id = {m}"),
        )
        .await,
        "match format is shorter than its existing maps",
    );
}

#[tokio::test]
async fn winners_must_be_teams_of_the_match() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let (m, c) = (s.bo3, s.team_c);

    assert_rejected(
        exec(
            &db.pool,
            format!("UPDATE matches SET winner_team_id = {c} WHERE id = {m}"),
        )
        .await,
        "CHECK constraint failed",
    );
    assert_rejected(
        exec(
            &db.pool,
            format!("INSERT INTO maps (match_id, number, winner_team_id) VALUES ({m}, 1, {c})"),
        )
        .await,
        "map winner is not a team of the match",
    );
    assert_rejected(
        store::picks::save(&db.pool, GUILD, ALICE, m, &winner_only(c), at(12, 0)).await,
        "pick winner is not a team of the match",
    );
    let map_pick = PickParts {
        maps: vec![MapPick {
            map_number: 1,
            winner_team_id: c,
            rounds: Some((13, 7)),
            overtime: false,
        }],
        ..PickParts::default()
    };
    assert_rejected(
        store::picks::save(&db.pool, GUILD, ALICE, m, &map_pick, at(12, 0)).await,
        "map pick winner is not a team of the match",
    );

    // A TBD slot accepts no winner, even one that may later fill it.
    let tbd = match_between(&db.pool, s.event, Some(s.team_a), None, MatchFormat::Bo1).await;
    assert_rejected(
        exec(
            &db.pool,
            format!("UPDATE matches SET winner_team_id = {c} WHERE id = {tbd}"),
        )
        .await,
        "CHECK constraint failed",
    );
    assert_rejected(
        store::picks::save(&db.pool, GUILD, ALICE, tbd, &winner_only(c), at(12, 0)).await,
        "pick winner is not a team of the match",
    );
}

#[tokio::test]
async fn teams_named_as_winner_cannot_leave_the_match() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    store::picks::save(
        &db.pool,
        GUILD,
        ALICE,
        s.bo3,
        &winner_only(s.team_a),
        at(12, 0),
    )
    .await
    .unwrap();
    assert_rejected(
        exec(
            &db.pool,
            format!(
                "UPDATE matches SET team_a_id = {} WHERE id = {}",
                s.team_c, s.bo3
            ),
        )
        .await,
        "cannot be removed from the match",
    );
    // Replacing the team nobody picked is fine.
    exec(
        &db.pool,
        format!(
            "UPDATE matches SET team_b_id = {} WHERE id = {}",
            s.team_c, s.bo3
        ),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn row_level_checks() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let (e, m, a) = (s.event, s.bo3, s.team_a);
    let pick = store::picks::save(&db.pool, GUILD, ALICE, m, &winner_only(a), at(12, 0))
        .await
        .unwrap();
    let p = pick.id;
    let cases = [
        // A match between a team and itself.
        format!(
            "INSERT INTO matches (event_id, format, team_a_id, team_b_id) VALUES ({e}, 'bo1', {a}, {a})"
        ),
        // More maps won than the format allows.
        format!("UPDATE matches SET maps_won_a = 3 WHERE id = {m}"),
        format!("UPDATE matches SET maps_won_b = -1 WHERE id = {m}"),
        // Half-set bracket feed.
        format!("UPDATE matches SET feeds_b_from_match_id = {m} WHERE id = {m}"),
        format!("UPDATE matches SET round_index = -1 WHERE id = {m}"),
        // Half-set map score.
        format!("INSERT INTO maps (match_id, number, rounds_a) VALUES ({m}, 1, 13)"),
        // Half-set or out-of-range series pick.
        format!("UPDATE picks SET series_a = 2 WHERE id = {p}"),
        format!("UPDATE picks SET series_a = 4, series_b = 0 WHERE id = {p}"),
        // Map pick: half-set score, or a score together with overtime.
        format!(
            "INSERT INTO pick_maps (pick_id, map_number, winner_team_id, rounds_a) VALUES ({p}, 1, {a}, 13)"
        ),
        format!(
            "INSERT INTO pick_maps (pick_id, map_number, winner_team_id, rounds_a, rounds_b, overtime) \
             VALUES ({p}, 1, {a}, 13, 5, 1)"
        ),
        // Non-numeric snowflakes and empty names.
        "INSERT INTO users (discord_user_id, username, updated_at) \
         VALUES ('abc', 'x', '2026-11-20T12:00:00.000Z')"
            .to_owned(),
        "INSERT INTO teams (name) VALUES ('')".to_owned(),
        // Invalid JSON.
        format!(
            "INSERT INTO pick_results (pick_id, points, breakdown, rules_version, settled_at) \
             VALUES ({p}, 3, '{{not json', 1, '2026-11-20T20:00:00.000Z')"
        ),
        "INSERT INTO source_records (source, entity_type, external_id, payload, payload_hash, \
           fetched_at) VALUES ('fixture', 'match', '1', 'nope', 'h', '2026-11-20T12:00:00.000Z')"
            .to_owned(),
    ];
    for sql in cases {
        let res = exec(&db.pool, sql.clone()).await;
        assert!(
            matches!(&res, Err(sqlx::Error::Database(d)) if d.message().contains("CHECK constraint failed")),
            "{sql}\n=> {res:?}"
        );
    }

    // STRICT tables reject values of the wrong type instead of storing them.
    assert_rejected(
        exec(
            &db.pool,
            format!("UPDATE matches SET round_index = 'first' WHERE id = {m}"),
        )
        .await,
        "cannot store TEXT value in INTEGER column",
    );

    // The valid forms of the same rows are accepted.
    store::picks::save(
        &db.pool,
        GUILD,
        ALICE,
        m,
        &PickParts {
            winner_team_id: Some(a),
            series: Some(SeriesScore { a: 2, b: 1 }),
            maps: vec![MapPick {
                map_number: 1,
                winner_team_id: a,
                rounds: None,
                overtime: true,
            }],
        },
        at(12, 5),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn foreign_keys_are_enforced() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let cases = [
        "INSERT INTO stages (event_id, name, kind) VALUES (999, 's', 'swiss')".to_owned(),
        "INSERT INTO matches (event_id, format) VALUES (999, 'bo1')".to_owned(),
        format!(
            "INSERT INTO matches (event_id, format, team_a_id) VALUES ({}, 'bo1', 999)",
            s.event
        ),
        format!(
            "INSERT INTO guild_members (guild_id, user_id, last_seen_at) \
             VALUES ('{GUILD}', '999', '2026-11-20T12:00:00.000Z')"
        ),
        format!(
            "INSERT INTO followed_events (guild_id, event_id, followed_by, followed_at) \
             VALUES ('{GUILD}', 999, '{ALICE}', '2026-11-20T12:00:00.000Z')"
        ),
        "INSERT INTO pick_revisions (pick_id, snapshot, edited_at) \
         VALUES (999, '{}', '2026-11-20T12:00:00.000Z')"
            .to_owned(),
    ];
    for sql in cases {
        assert_rejected(exec(&db.pool, sql).await, "FOREIGN KEY constraint failed");
    }
    // A pick needs a known user and match.
    assert_rejected(
        store::picks::save(
            &db.pool,
            GUILD,
            "999",
            s.bo3,
            &winner_only(s.team_a),
            at(12, 0),
        )
        .await,
        "FOREIGN KEY constraint failed",
    );
    assert_rejected(
        store::picks::save(
            &db.pool,
            GUILD,
            ALICE,
            domain::MatchId(999),
            &PickParts::default(),
            at(12, 0),
        )
        .await,
        "FOREIGN KEY constraint failed",
    );
}

#[tokio::test]
async fn external_ids_are_unique_per_source_and_type() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let link = |entity_id: i64, source: &str| ExternalRef {
        entity_type: EntityType::Team,
        entity_id,
        source: source.to_owned(),
        external_id: "ps-42".to_owned(),
    };
    refs::link(&db.pool, &link(s.team_a.0, "pandascore"))
        .await
        .unwrap();
    assert_rejected(
        refs::link(&db.pool, &link(s.team_b.0, "pandascore")).await,
        "UNIQUE constraint failed",
    );
    // The same external ID from another source, or for another entity type,
    // is a different reference.
    refs::link(&db.pool, &link(s.team_b.0, "fixture"))
        .await
        .unwrap();
    refs::link(
        &db.pool,
        &ExternalRef {
            entity_type: EntityType::Match,
            ..link(s.bo3.0, "pandascore")
        },
    )
    .await
    .unwrap();
}
