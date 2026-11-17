//! Matches and their maps.

use chrono::{DateTime, Utc};
use domain::{
    BracketPos, EventId, Feed, Match, MatchFields, MatchId, MatchMap, NewMatch, StageId, TeamId,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::row::{enum_col, opt_enum_col};
use crate::{DbResult, Ts};

/// Every column but `id`, in the order [`bind_fields!`] binds them.
macro_rules! field_columns {
    () => {
        "event_id, stage_id, team_a_id, team_b_id, format, status, \
    scheduled_at, started_at, ended_at, lock_at, locked, voided, maps_won_a, maps_won_b, \
    winner_team_id, bracket_side, round_index, slot_index, \
    feeds_a_from_match_id, feeds_a_from_outcome, feeds_b_from_match_id, feeds_b_from_outcome"
    };
}

macro_rules! map_columns {
    () => {
        "match_id, number, map_name, status, rounds_a, rounds_b, winner_team_id"
    };
}

/// Binds all [`MatchFields`] in `field_columns!` order.
macro_rules! bind_fields {
    ($query:expr, $f:expr) => {{
        let f: &MatchFields = $f;
        let b = &f.bracket;
        $query
            .bind(f.event_id.0)
            .bind(f.stage_id.map(|id| id.0))
            .bind(f.team_a_id.map(|id| id.0))
            .bind(f.team_b_id.map(|id| id.0))
            .bind(f.format.as_str())
            .bind(f.status.as_str())
            .bind(Ts::opt(f.scheduled_at))
            .bind(Ts::opt(f.started_at))
            .bind(Ts::opt(f.ended_at))
            .bind(Ts::opt(f.lock_at))
            .bind(f.locked)
            .bind(f.voided)
            .bind(f.maps_won_a)
            .bind(f.maps_won_b)
            .bind(f.winner_team_id.map(|id| id.0))
            .bind(b.side.map(|s| s.as_str()))
            .bind(b.round_index)
            .bind(b.slot_index)
            .bind(b.feeds_a_from.map(|feed| feed.match_id.0))
            .bind(b.feeds_a_from.map(|feed| feed.outcome.as_str()))
            .bind(b.feeds_b_from.map(|feed| feed.match_id.0))
            .bind(b.feeds_b_from.map(|feed| feed.outcome.as_str()))
    }};
}

fn feed_from_row(row: &SqliteRow, id_col: &str, outcome_col: &str) -> DbResult<Option<Feed>> {
    let id: Option<i64> = row.try_get(id_col)?;
    let outcome = opt_enum_col(row, outcome_col)?;
    // The schema guarantees both are set or both are NULL.
    Ok(id.zip(outcome).map(|(id, outcome)| Feed {
        match_id: MatchId(id),
        outcome,
    }))
}

fn from_row(row: SqliteRow) -> DbResult<Match> {
    let team = |col: &str| -> DbResult<Option<TeamId>> {
        Ok(row.try_get::<Option<i64>, _>(col)?.map(TeamId))
    };
    Ok(Match {
        id: MatchId(row.try_get("id")?),
        fields: MatchFields {
            event_id: EventId(row.try_get("event_id")?),
            stage_id: row.try_get::<Option<i64>, _>("stage_id")?.map(StageId),
            team_a_id: team("team_a_id")?,
            team_b_id: team("team_b_id")?,
            format: enum_col(&row, "format")?,
            status: enum_col(&row, "status")?,
            scheduled_at: row.try_get("scheduled_at")?,
            started_at: row.try_get("started_at")?,
            ended_at: row.try_get("ended_at")?,
            lock_at: row.try_get("lock_at")?,
            locked: row.try_get("locked")?,
            voided: row.try_get("voided")?,
            maps_won_a: row.try_get("maps_won_a")?,
            maps_won_b: row.try_get("maps_won_b")?,
            winner_team_id: team("winner_team_id")?,
            bracket: BracketPos {
                side: opt_enum_col(&row, "bracket_side")?,
                round_index: row.try_get("round_index")?,
                slot_index: row.try_get("slot_index")?,
                feeds_a_from: feed_from_row(&row, "feeds_a_from_match_id", "feeds_a_from_outcome")?,
                feeds_b_from: feed_from_row(&row, "feeds_b_from_match_id", "feeds_b_from_outcome")?,
            },
        },
    })
}

fn map_from_row(row: SqliteRow) -> DbResult<MatchMap> {
    Ok(MatchMap {
        match_id: MatchId(row.try_get("match_id")?),
        number: row.try_get("number")?,
        map_name: row.try_get("map_name")?,
        status: enum_col(&row, "status")?,
        rounds_a: row.try_get("rounds_a")?,
        rounds_b: row.try_get("rounds_b")?,
        winner_team_id: row.try_get::<Option<i64>, _>("winner_team_id")?.map(TeamId),
    })
}

pub async fn insert(ex: impl SqliteExecutor<'_>, m: &NewMatch) -> DbResult<MatchId> {
    let sql = concat!(
        "INSERT INTO matches (",
        field_columns!(),
        ") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
    );
    let id: i64 = bind_fields!(sqlx::query_scalar(sql), m)
        .fetch_one(ex)
        .await?;
    Ok(MatchId(id))
}

/// Overwrites every field of an existing match. Returns `false` if it does
/// not exist. Team A/B orientation is the caller's responsibility: never swap
/// the teams of an existing match (see the key invariants).
pub async fn update(ex: impl SqliteExecutor<'_>, m: &Match) -> DbResult<bool> {
    // Same order as `field_columns!`.
    let sql = "UPDATE matches SET event_id = ?, stage_id = ?, team_a_id = ?, team_b_id = ?, \
        format = ?, status = ?, scheduled_at = ?, started_at = ?, ended_at = ?, lock_at = ?, \
        locked = ?, voided = ?, maps_won_a = ?, maps_won_b = ?, winner_team_id = ?, bracket_side = ?, \
        round_index = ?, slot_index = ?, feeds_a_from_match_id = ?, feeds_a_from_outcome = ?, \
        feeds_b_from_match_id = ?, feeds_b_from_outcome = ? WHERE id = ?";
    let res = bind_fields!(sqlx::query(sql), &m.fields)
        .bind(m.id.0)
        .execute(ex)
        .await?;
    Ok(res.rows_affected() == 1)
}

/// Locks the picks of every unlocked match whose `lock_at` has passed or
/// that is already live or finished (T10 lock rule, run by the lock ticker
/// in T06). Returns the IDs it locked; running it again locks nothing new.
pub async fn lock_due(ex: impl SqliteExecutor<'_>, now: DateTime<Utc>) -> DbResult<Vec<MatchId>> {
    sqlx::query_scalar(
        "UPDATE matches SET locked = 1 \
         WHERE locked = 0 AND (lock_at <= ? OR status IN ('live', 'finished')) \
         RETURNING id",
    )
    .bind(Ts(now))
    .fetch_all(ex)
    .await
    .map(|ids: Vec<i64>| {
        let mut ids: Vec<MatchId> = ids.into_iter().map(MatchId).collect();
        ids.sort_by_key(|id| id.0);
        ids
    })
}

pub async fn get(ex: impl SqliteExecutor<'_>, id: MatchId) -> DbResult<Option<Match>> {
    sqlx::query(concat!(
        "SELECT id, ",
        field_columns!(),
        " FROM matches WHERE id = ?"
    ))
    .bind(id.0)
    .try_map(from_row)
    .fetch_optional(ex)
    .await
}

/// The matches of an event by scheduled time; unscheduled ones last.
pub async fn list_for_event(
    ex: impl SqliteExecutor<'_>,
    event_id: EventId,
) -> DbResult<Vec<Match>> {
    sqlx::query(concat!(
        "SELECT id, ",
        field_columns!(),
        " FROM matches WHERE event_id = ? ORDER BY scheduled_at IS NULL, scheduled_at, id"
    ))
    .bind(event_id.0)
    .try_map(from_row)
    .fetch_all(ex)
    .await
}

/// Matches of an event between two teams (in either order) scheduled within
/// `[from, to]`, closest to the middle of the window first. For cross-source
/// identity (T03).
pub async fn find_between(
    ex: impl SqliteExecutor<'_>,
    event_id: EventId,
    team1: TeamId,
    team2: TeamId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> DbResult<Vec<Match>> {
    let mid = from + (to - from) / 2;
    sqlx::query(concat!(
        "SELECT id, ",
        field_columns!(),
        " FROM matches WHERE event_id = ?1 \
           AND ((team_a_id = ?2 AND team_b_id = ?3) OR (team_a_id = ?3 AND team_b_id = ?2)) \
           AND scheduled_at BETWEEN ?4 AND ?5 \
         ORDER BY abs(julianday(scheduled_at) - julianday(?6)), id"
    ))
    .bind(event_id.0)
    .bind(team1.0)
    .bind(team2.0)
    .bind(Ts(from))
    .bind(Ts(to))
    .bind(Ts(mid))
    .try_map(from_row)
    .fetch_all(ex)
    .await
}

/// Inserts a map, or overwrites the map with the same match and number.
pub async fn upsert_map(ex: impl SqliteExecutor<'_>, map: &MatchMap) -> DbResult<()> {
    sqlx::query(concat!(
        "INSERT INTO maps (",
        map_columns!(),
        ") VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (match_id, number) DO UPDATE SET \
           map_name = excluded.map_name, status = excluded.status, \
           rounds_a = excluded.rounds_a, rounds_b = excluded.rounds_b, \
           winner_team_id = excluded.winner_team_id"
    ))
    .bind(map.match_id.0)
    .bind(map.number)
    .bind(&map.map_name)
    .bind(map.status.as_str())
    .bind(map.rounds_a)
    .bind(map.rounds_b)
    .bind(map.winner_team_id.map(|id| id.0))
    .execute(ex)
    .await?;
    Ok(())
}

/// The maps of a match, by number.
pub async fn list_maps(ex: impl SqliteExecutor<'_>, match_id: MatchId) -> DbResult<Vec<MatchMap>> {
    sqlx::query(concat!(
        "SELECT ",
        map_columns!(),
        " FROM maps WHERE match_id = ? ORDER BY number"
    ))
    .bind(match_id.0)
    .try_map(map_from_row)
    .fetch_all(ex)
    .await
}
