//! Shared helpers for the store integration tests: a migrated temp-file
//! database and a small seed (one event, three teams, one Bo3 match).

// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use domain::{
    EventId, EventStatus, EventTier, MatchFields, MatchFormat, MatchId, NewEvent, NewTeam, TeamId,
};
use store::SqlitePool;
use store::users::User;

pub struct TestDb {
    pub pool: SqlitePool,
    pub path: PathBuf,
    // Dropped last: deletes the directory with the database file.
    _dir: tempfile::TempDir,
}

impl TestDb {
    pub fn url(&self) -> String {
        format!("sqlite://{}", self.path.display())
    }
}

/// A fresh, migrated database in a temp directory.
pub async fn test_db() -> TestDb {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let pool = store::open(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    TestDb {
        pool,
        path,
        _dir: dir,
    }
}

/// A fixed time on 2026-11-20: `at(h, m)`.
pub fn at(hour: u32, min: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, hour, min, 0).unwrap()
}

pub const GUILD: &str = "100000000000000001";
pub const ALICE: &str = "200000000000000001";
pub const BOB: &str = "200000000000000002";

pub struct Seed {
    pub event: EventId,
    pub team_a: TeamId,
    pub team_b: TeamId,
    /// A team that does not play in `bo3`.
    pub team_c: TeamId,
    /// A Bo3 between `team_a` and `team_b` at 18:00.
    pub bo3: MatchId,
}

pub async fn team(pool: &SqlitePool, name: &str) -> TeamId {
    store::teams::insert(
        pool,
        &NewTeam {
            name: name.to_owned(),
            acronym: None,
            logo_url: None,
            country: None,
        },
    )
    .await
    .unwrap()
}

pub async fn event(pool: &SqlitePool, name: &str) -> EventId {
    store::events::insert(
        pool,
        &NewEvent {
            name: name.to_owned(),
            tier: EventTier::S,
            begin_at: Some(at(10, 0)),
            end_at: None,
            status: EventStatus::Upcoming,
        },
    )
    .await
    .unwrap()
}

pub async fn match_between(
    pool: &SqlitePool,
    event: EventId,
    a: Option<TeamId>,
    b: Option<TeamId>,
    format: MatchFormat,
) -> MatchId {
    store::matches::insert(
        pool,
        &MatchFields::scheduled(event, a, b, format, Some(at(18, 0))),
    )
    .await
    .unwrap()
}

pub async fn user(pool: &SqlitePool, id: &str) {
    store::users::upsert_user(
        pool,
        &User {
            discord_user_id: id.to_owned(),
            username: format!("user{id}"),
            global_name: None,
            avatar_hash: None,
            updated_at: at(9, 0),
        },
    )
    .await
    .unwrap();
}

pub async fn seed(pool: &SqlitePool) -> Seed {
    let event = event(pool, "IEM Test").await;
    let team_a = team(pool, "Alpha").await;
    let team_b = team(pool, "Bravo").await;
    let team_c = team(pool, "Charlie").await;
    let bo3 = match_between(pool, event, Some(team_a), Some(team_b), MatchFormat::Bo3).await;
    user(pool, ALICE).await;
    user(pool, BOB).await;
    Seed {
        event,
        team_a,
        team_b,
        team_c,
        bo3,
    }
}

/// Runs a raw statement, for constraint tests that bypass the repositories.
pub async fn exec(pool: &SqlitePool, sql: impl Into<String>) -> Result<(), sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(sql.into()))
        .execute(pool)
        .await
        .map(|_| ())
}

/// Asserts that `res` is a database error whose message contains `needle`.
#[track_caller]
pub fn assert_rejected<T: std::fmt::Debug>(res: Result<T, sqlx::Error>, needle: &str) {
    match res {
        Err(sqlx::Error::Database(e)) => assert!(
            e.message().contains(needle),
            "expected an error containing `{needle}`, got `{}`",
            e.message()
        ),
        other => panic!("expected a database error containing `{needle}`, got {other:?}"),
    }
}
