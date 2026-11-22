//! The leaderboard query (T14) against a temp database: it reads
//! `pick_results` only, filters by guild and event, and names members.

mod common;

use std::collections::HashMap;

use chrono::{DateTime, Duration};
use common::{ALICE, BOB, GUILD, at, event, match_between, seed, team, test_db, user};
use domain::{MatchFormat, MatchId, PickParts, ResultState};
use serde_json::{Value, json};
use store::guilds::{FollowedEvent, GuildSettings};
use store::picks::PickResult;
use store::users::GuildMember;
use store::{SqlitePool, guilds, leaderboard, matches, picks, scoring, users};

const CAROL: &str = "200000000000000003";
const DAVE: &str = "200000000000000004";
const OTHER_GUILD: &str = "100000000000000002";

/// A stored breakdown: the winner part, an optional series part and the
/// outcomes of map-score parts.
fn breakdown(winner: &str, series: Option<&str>, map_scores: &[&str]) -> Value {
    let part =
        |o: &str, pts: u32| json!({ "outcome": o, "points": if o == "hit" { pts } else { 0 } });
    json!({
        "winner": part(winner, 3),
        "series": series.map(|o| part(o, 3)),
        "maps": map_scores.iter().enumerate().map(|(i, o)| json!({
            "map_number": i + 1,
            "winner": part("hit", 1),
            "score": part(o, 2),
        })).collect::<Vec<_>>(),
    })
}

async fn settle(
    pool: &SqlitePool,
    guild: &str,
    user_id: &str,
    m: MatchId,
    points: i64,
    state: ResultState,
    breakdown: Value,
) {
    let p = picks::save(pool, guild, user_id, m, &PickParts::default(), at(9, 0))
        .await
        .unwrap();
    picks::upsert_result(
        pool,
        &PickResult {
            pick_id: p.id,
            points,
            breakdown,
            rules_version: 1,
            settled_at: at(22, 0),
            state,
            pending_parts: 0,
        },
    )
    .await
    .unwrap();
}

async fn start_at(pool: &SqlitePool, id: MatchId, t: DateTime<chrono::Utc>) {
    let mut m = matches::get(pool, id).await.unwrap().unwrap();
    m.fields.started_at = Some(t);
    matches::update(pool, &m).await.unwrap();
}

async fn follow(pool: &SqlitePool, guild: &str, ev: domain::EventId) {
    guilds::follow_event(
        pool,
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

/// Points per member straight from `pick_results`.
async fn sums(pool: &SqlitePool, guild: &str) -> HashMap<String, i64> {
    sqlx::query_as::<_, (String, i64)>(
        "SELECT p.user_id, sum(r.points) FROM pick_results r JOIN picks p ON p.id = r.pick_id \
         WHERE p.guild_id = ? GROUP BY p.user_id",
    )
    .bind(guild)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

#[tokio::test]
async fn points_equal_the_sum_of_pick_results() {
    let db = test_db().await;
    let pool = &db.pool;
    let s = seed(pool).await;
    user(pool, CAROL).await;
    user(pool, DAVE).await;
    let ev2 = event(pool, "Major").await;
    let c = team(pool, "Delta").await;
    let bo1 = match_between(pool, ev2, Some(s.team_a), Some(c), MatchFormat::Bo1).await;
    let bo5 = match_between(pool, s.event, Some(s.team_b), Some(c), MatchFormat::Bo5).await;
    let canceled = match_between(pool, ev2, Some(s.team_b), Some(c), MatchFormat::Bo3).await;

    // Final results of every kind, one live (provisional) match, and a void
    // one: Dave has only void results.
    settle(
        pool,
        GUILD,
        ALICE,
        s.bo3,
        11,
        ResultState::Final,
        breakdown("hit", Some("hit"), &["hit", "miss"]),
    )
    .await;
    settle(
        pool,
        GUILD,
        BOB,
        s.bo3,
        3,
        ResultState::Final,
        breakdown("hit", Some("miss"), &["miss"]),
    )
    .await;
    settle(
        pool,
        GUILD,
        CAROL,
        s.bo3,
        0,
        ResultState::Final,
        breakdown("miss", Some("miss"), &[]),
    )
    .await;
    settle(
        pool,
        GUILD,
        ALICE,
        bo1,
        6,
        ResultState::Final,
        breakdown("hit", None, &["hit"]),
    )
    .await;
    settle(
        pool,
        GUILD,
        CAROL,
        bo1,
        4,
        ResultState::Final,
        breakdown("pending", None, &["pending"]),
    )
    .await;
    settle(
        pool,
        GUILD,
        BOB,
        bo5,
        4,
        ResultState::Provisional,
        breakdown("pending", Some("pending"), &["hit", "pending"]),
    )
    .await;
    settle(
        pool,
        GUILD,
        CAROL,
        bo5,
        1,
        ResultState::Provisional,
        breakdown("pending", Some("pending"), &["miss"]),
    )
    .await;
    settle(
        pool,
        GUILD,
        ALICE,
        canceled,
        0,
        ResultState::Void,
        breakdown("void", Some("void"), &["void"]),
    )
    .await;
    settle(
        pool,
        GUILD,
        DAVE,
        canceled,
        0,
        ResultState::Void,
        breakdown("void", None, &[]),
    )
    .await;
    // Another guild doesn't count here.
    settle(
        pool,
        OTHER_GUILD,
        BOB,
        s.bo3,
        15,
        ResultState::Final,
        breakdown("hit", Some("hit"), &["hit"]),
    )
    .await;

    let lb = leaderboard::load(pool, GUILD, None).await.unwrap();
    let sums = sums(pool, GUILD).await;
    let rows: Vec<_> = lb.board.rows.iter().map(|r| &r.standing).collect();
    for s in &rows {
        assert_eq!(s.points, sums[&s.user_id], "points of {}", s.user_id);
    }
    // Every member with a non-void result is on the board; Dave isn't.
    assert_eq!(rows.len(), 3);
    assert_eq!(sums[DAVE], 0);
    // `totals_for_guild` agrees on everyone on the board.
    for t in scoring::totals_for_guild(pool, GUILD).await.unwrap() {
        if let Some(s) = rows.iter().find(|s| s.user_id == t.user_id) {
            assert_eq!(s.points, t.points);
        }
    }

    // Alice 17, Bob 7 (4 live), Carol 5 (1 live).
    let summary: Vec<_> = lb
        .board
        .rows
        .iter()
        .map(|r| {
            (
                r.standing.user_id.as_str(),
                r.rank,
                r.standing.points,
                r.standing.provisional_points,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [(ALICE, 1, 17, 0), (BOB, 2, 7, 4), (CAROL, 3, 5, 1)]
    );
    let alice = &lb.board.rows[0].standing;
    assert_eq!(
        (
            alice.settled_picks,
            alice.winner_hits,
            alice.winner_decided,
            alice.series_hits,
            alice.map_hits
        ),
        (2, 2, 2, 1, 2)
    );
    let carol = &lb.board.rows[2].standing;
    assert_eq!(
        (carol.settled_picks, carol.winner_hits, carol.winner_decided),
        (3, 0, 1)
    );

    // One event: only its matches.
    let lb = leaderboard::load(pool, GUILD, Some(ev2)).await.unwrap();
    let summary: Vec<_> = lb
        .board
        .rows
        .iter()
        .map(|r| (r.standing.user_id.as_str(), r.rank, r.standing.points))
        .collect();
    assert_eq!(summary, [(ALICE, 1, 6), (CAROL, 2, 4)]);

    // The other guild sees only its own.
    let lb = leaderboard::load(pool, OTHER_GUILD, None).await.unwrap();
    assert_eq!(lb.board.rows.len(), 1);
    assert_eq!(lb.board.rows[0].standing.points, 15);
}

#[tokio::test]
async fn members_carry_display_names_and_avatars() {
    let db = test_db().await;
    let pool = &db.pool;
    let s = seed(pool).await;
    let mut bob = users::get_user(pool, BOB).await.unwrap().unwrap();
    bob.global_name = Some("Bobby".into());
    bob.avatar_hash = Some("abc".into());
    users::upsert_user(pool, &bob).await.unwrap();
    users::upsert_member(
        pool,
        &GuildMember {
            guild_id: GUILD.into(),
            user_id: ALICE.into(),
            nick: Some("Ali".into()),
            last_seen_at: at(9, 0),
        },
    )
    .await
    .unwrap();
    settle(
        pool,
        GUILD,
        ALICE,
        s.bo3,
        3,
        ResultState::Final,
        breakdown("hit", None, &[]),
    )
    .await;
    settle(
        pool,
        GUILD,
        BOB,
        s.bo3,
        3,
        ResultState::Final,
        breakdown("hit", None, &[]),
    )
    .await;

    let lb = leaderboard::load(pool, GUILD, None).await.unwrap();
    assert_eq!(lb.name_of(ALICE), "Ali");
    assert_eq!(lb.name_of(BOB), "Bobby");
    assert_eq!(lb.members[BOB].avatar_hash.as_deref(), Some("abc"));
    assert_eq!(lb.members[ALICE].avatar_hash, None);
    // A full tie shares rank 1, ordered by name.
    let rows: Vec<_> = lb
        .board
        .rows
        .iter()
        .map(|r| (r.standing.user_id.as_str(), r.rank))
        .collect();
    assert_eq!(rows, [(ALICE, 1), (BOB, 1)]);
}

#[tokio::test]
async fn match_days_use_the_guild_time_zone() {
    let db = test_db().await;
    let pool = &db.pool;
    let s = seed(pool).await;
    let late = match_between(
        pool,
        s.event,
        Some(s.team_a),
        Some(s.team_c),
        MatchFormat::Bo1,
    )
    .await;
    // 12:00 and 23:30 UTC on 20 Nov: two days in Berlin, one in UTC.
    start_at(pool, s.bo3, at(12, 0)).await;
    start_at(pool, late, at(23, 30)).await;
    settle(
        pool,
        GUILD,
        ALICE,
        s.bo3,
        3,
        ResultState::Final,
        breakdown("hit", None, &[]),
    )
    .await;
    settle(
        pool,
        GUILD,
        BOB,
        late,
        6,
        ResultState::Final,
        breakdown("hit", None, &["hit"]),
    )
    .await;

    // Default (Europe/Berlin): the late match is on 21 Nov; Bob is new.
    let lb = leaderboard::load(pool, GUILD, None).await.unwrap();
    assert_eq!(
        lb.board.match_day,
        Some(at(0, 0).date_naive() + Duration::days(1))
    );
    assert!(lb.board.has_previous);
    let rows: Vec<_> = lb
        .board
        .rows
        .iter()
        .map(|r| (r.standing.user_id.as_str(), r.rank, r.previous_rank))
        .collect();
    assert_eq!(rows, [(BOB, 1, None), (ALICE, 2, Some(1))]);

    // In UTC both are on 20 Nov: no previous standing.
    guilds::upsert_settings(
        pool,
        &GuildSettings {
            timezone: "UTC".into(),
            ..GuildSettings::new(GUILD, at(9, 0))
        },
    )
    .await
    .unwrap();
    let lb = leaderboard::load(pool, GUILD, None).await.unwrap();
    assert_eq!(lb.board.match_day, Some(at(0, 0).date_naive()));
    assert!(!lb.board.has_previous);
}

#[tokio::test]
async fn filter_events_are_the_followed_ones() {
    let db = test_db().await;
    let pool = &db.pool;
    let s = seed(pool).await;
    let ev2 = event(pool, "Major").await;
    let elsewhere = event(pool, "Elsewhere").await;
    follow(pool, GUILD, s.event).await;
    follow(pool, GUILD, ev2).await;
    follow(pool, OTHER_GUILD, elsewhere).await;
    let names: Vec<_> = leaderboard::filter_events(pool, GUILD)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    // Same start: by name.
    assert_eq!(names, ["IEM Test", "Major"]);
}

#[tokio::test]
async fn an_unreadable_breakdown_still_counts_its_points() {
    let db = test_db().await;
    let pool = &db.pool;
    let s = seed(pool).await;
    settle(pool, GUILD, ALICE, s.bo3, 5, ResultState::Final, json!({})).await;
    let lb = leaderboard::load(pool, GUILD, None).await.unwrap();
    assert_eq!(lb.board.rows[0].standing.points, 5);
    assert_eq!(lb.board.rows[0].standing.winner_decided, 0);
}
