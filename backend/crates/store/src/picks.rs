//! Picks, their map parts, edit history and settled results.
//!
//! Validation against the pick rules (formats, lock) is the caller's job
//! (T10); the schema only guards the structural invariants.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use domain::{MapPick, MatchId, Pick, PickId, PickParts, SeriesScore, TeamId};
use serde_json::{Value, json};
use sqlx::sqlite::SqliteRow;
use sqlx::types::Json;
use sqlx::{Acquire, Row, Sqlite, SqliteConnection, SqliteExecutor};

use crate::{DbResult, Ts};

/// One saved version of a pick.
#[derive(Debug, Clone, PartialEq)]
pub struct PickRevision {
    pub id: i64,
    pub pick_id: PickId,
    /// The pick's parts as saved; see [`snapshot`].
    pub snapshot: Value,
    pub edited_at: DateTime<Utc>,
}

/// The settled points of one pick (T11).
#[derive(Debug, Clone, PartialEq)]
pub struct PickResult {
    pub pick_id: PickId,
    pub points: i64,
    pub breakdown: Value,
    pub rules_version: i64,
    pub settled_at: DateTime<Utc>,
}

/// JSON form of a pick's parts, as stored in `pick_revisions.snapshot`.
pub fn snapshot(parts: &PickParts) -> Value {
    json!({
        "winner_team_id": parts.winner_team_id.map(|t| t.0),
        "series": parts.series.map(|s| json!({ "a": s.a, "b": s.b })),
        "maps": parts.maps.iter().map(|m| json!({
            "map_number": m.map_number,
            "winner_team_id": m.winner_team_id.0,
            "rounds": m.rounds.map(|(a, b)| json!([a, b])),
            "overtime": m.overtime,
        })).collect::<Vec<_>>(),
    })
}

macro_rules! pick_columns {
    () => {
        "id, guild_id, user_id, match_id, winner_team_id, series_a, series_b, \
         created_at, updated_at"
    };
}

/// A pick row without its map parts.
fn pick_from_row(row: SqliteRow) -> DbResult<Pick> {
    let series_a: Option<u8> = row.try_get("series_a")?;
    let series_b: Option<u8> = row.try_get("series_b")?;
    Ok(Pick {
        id: PickId(row.try_get("id")?),
        guild_id: row.try_get("guild_id")?,
        user_id: row.try_get("user_id")?,
        match_id: MatchId(row.try_get("match_id")?),
        parts: PickParts {
            winner_team_id: row.try_get::<Option<i64>, _>("winner_team_id")?.map(TeamId),
            series: series_a.zip(series_b).map(|(a, b)| SeriesScore { a, b }),
            maps: Vec::new(),
        },
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_pick_from_row(row: SqliteRow) -> DbResult<(PickId, MapPick)> {
    let rounds_a: Option<u16> = row.try_get("rounds_a")?;
    let rounds_b: Option<u16> = row.try_get("rounds_b")?;
    Ok((
        PickId(row.try_get("pick_id")?),
        MapPick {
            map_number: row.try_get("map_number")?,
            winner_team_id: TeamId(row.try_get("winner_team_id")?),
            rounds: rounds_a.zip(rounds_b),
            overtime: row.try_get("overtime")?,
        },
    ))
}

/// Creates or replaces the caller's pick for a match in one transaction:
/// upserts the pick, replaces its map parts and appends a revision.
///
/// Removing every part is a delete ([`delete`]), not a save; the caller
/// decides (T10).
pub async fn save<'a, A>(
    conn: A,
    guild_id: &str,
    user_id: &str,
    match_id: MatchId,
    parts: &PickParts,
    now: DateTime<Utc>,
) -> DbResult<Pick>
where
    A: Acquire<'a, Database = Sqlite>,
{
    let mut tx = conn.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO picks (guild_id, user_id, match_id, winner_team_id, series_a, series_b, \
           created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (guild_id, user_id, match_id) DO UPDATE SET \
           winner_team_id = excluded.winner_team_id, series_a = excluded.series_a, \
           series_b = excluded.series_b, updated_at = excluded.updated_at \
         RETURNING id",
    )
    .bind(guild_id)
    .bind(user_id)
    .bind(match_id.0)
    .bind(parts.winner_team_id.map(|t| t.0))
    .bind(parts.series.map(|s| s.a))
    .bind(parts.series.map(|s| s.b))
    .bind(Ts(now))
    .bind(Ts(now))
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM pick_maps WHERE pick_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for m in &parts.maps {
        sqlx::query(
            "INSERT INTO pick_maps (pick_id, map_number, winner_team_id, rounds_a, rounds_b, \
               overtime) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(m.map_number)
        .bind(m.winner_team_id.0)
        .bind(m.rounds.map(|r| r.0))
        .bind(m.rounds.map(|r| r.1))
        .bind(m.overtime)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("INSERT INTO pick_revisions (pick_id, snapshot, edited_at) VALUES (?, ?, ?)")
        .bind(id)
        .bind(Json(snapshot(parts)))
        .bind(Ts(now))
        .execute(&mut *tx)
        .await?;

    // Read back, so the result reflects what the database stored (sorted
    // maps, truncated timestamps).
    let pick = get_by_id(&mut tx, PickId(id)).await?;
    tx.commit().await?;
    pick.ok_or(sqlx::Error::RowNotFound)
}

async fn get_by_id(conn: &mut SqliteConnection, id: PickId) -> DbResult<Option<Pick>> {
    let Some(mut pick) = sqlx::query(concat!(
        "SELECT ",
        pick_columns!(),
        " FROM picks WHERE id = ?"
    ))
    .bind(id.0)
    .try_map(pick_from_row)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    pick.parts.maps = sqlx::query(
        "SELECT pick_id, map_number, winner_team_id, rounds_a, rounds_b, overtime \
         FROM pick_maps WHERE pick_id = ? ORDER BY map_number",
    )
    .bind(id.0)
    .try_map(map_pick_from_row)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|(_, m)| m)
    .collect();
    Ok(Some(pick))
}

/// A member's pick for a match, with its map parts.
pub async fn get<'a, A>(
    conn: A,
    guild_id: &str,
    user_id: &str,
    match_id: MatchId,
) -> DbResult<Option<Pick>>
where
    A: Acquire<'a, Database = Sqlite>,
{
    // A transaction gives both reads one consistent snapshot.
    let mut tx = conn.begin().await?;
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM picks WHERE guild_id = ? AND user_id = ? AND match_id = ?",
    )
    .bind(guild_id)
    .bind(user_id)
    .bind(match_id.0)
    .fetch_optional(&mut *tx)
    .await?;
    let pick = match id {
        Some(id) => get_by_id(&mut tx, PickId(id)).await?,
        None => None,
    };
    tx.commit().await?;
    Ok(pick)
}

/// Every pick of a guild for a match, with map parts, oldest first.
pub async fn list_for_match<'a, A>(
    conn: A,
    guild_id: &str,
    match_id: MatchId,
) -> DbResult<Vec<Pick>>
where
    A: Acquire<'a, Database = Sqlite>,
{
    let mut tx = conn.begin().await?;
    let mut picks: Vec<Pick> = sqlx::query(concat!(
        "SELECT ",
        pick_columns!(),
        " FROM picks WHERE guild_id = ? AND match_id = ? ORDER BY created_at, id"
    ))
    .bind(guild_id)
    .bind(match_id.0)
    .try_map(pick_from_row)
    .fetch_all(&mut *tx)
    .await?;

    let maps = sqlx::query(
        "SELECT pm.pick_id, pm.map_number, pm.winner_team_id, pm.rounds_a, pm.rounds_b, \
           pm.overtime \
         FROM pick_maps pm JOIN picks p ON p.id = pm.pick_id \
         WHERE p.guild_id = ? AND p.match_id = ? ORDER BY pm.pick_id, pm.map_number",
    )
    .bind(guild_id)
    .bind(match_id.0)
    .try_map(map_pick_from_row)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let index: HashMap<PickId, usize> = picks.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    for (pick_id, m) in maps {
        if let Some(&i) = index.get(&pick_id) {
            picks[i].parts.maps.push(m);
        }
    }
    Ok(picks)
}

/// Deletes a member's pick with its map parts, revisions and result.
/// Returns `false` if there was none.
pub async fn delete(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    user_id: &str,
    match_id: MatchId,
) -> DbResult<bool> {
    let res = sqlx::query("DELETE FROM picks WHERE guild_id = ? AND user_id = ? AND match_id = ?")
        .bind(guild_id)
        .bind(user_id)
        .bind(match_id.0)
        .execute(ex)
        .await?;
    Ok(res.rows_affected() == 1)
}

/// The edit history of a pick, oldest first.
pub async fn list_revisions(
    ex: impl SqliteExecutor<'_>,
    pick_id: PickId,
) -> DbResult<Vec<PickRevision>> {
    sqlx::query(
        "SELECT id, pick_id, snapshot, edited_at FROM pick_revisions \
         WHERE pick_id = ? ORDER BY id",
    )
    .bind(pick_id.0)
    .try_map(|row: SqliteRow| {
        Ok(PickRevision {
            id: row.try_get("id")?,
            pick_id: PickId(row.try_get("pick_id")?),
            snapshot: row.try_get::<Json<Value>, _>("snapshot")?.0,
            edited_at: row.try_get("edited_at")?,
        })
    })
    .fetch_all(ex)
    .await
}

/// Inserts or replaces the settled result of a pick. Settlement recomputes
/// and overwrites, never adds (T11).
pub async fn upsert_result(ex: impl SqliteExecutor<'_>, r: &PickResult) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO pick_results (pick_id, points, breakdown, rules_version, settled_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT (pick_id) DO UPDATE SET points = excluded.points, \
           breakdown = excluded.breakdown, rules_version = excluded.rules_version, \
           settled_at = excluded.settled_at",
    )
    .bind(r.pick_id.0)
    .bind(r.points)
    .bind(Json(&r.breakdown))
    .bind(r.rules_version)
    .bind(Ts(r.settled_at))
    .execute(ex)
    .await?;
    Ok(())
}

pub async fn get_result(
    ex: impl SqliteExecutor<'_>,
    pick_id: PickId,
) -> DbResult<Option<PickResult>> {
    sqlx::query(
        "SELECT pick_id, points, breakdown, rules_version, settled_at FROM pick_results \
         WHERE pick_id = ?",
    )
    .bind(pick_id.0)
    .try_map(|row: SqliteRow| {
        Ok(PickResult {
            pick_id: PickId(row.try_get("pick_id")?),
            points: row.try_get("points")?,
            breakdown: row.try_get::<Json<Value>, _>("breakdown")?.0,
            rules_version: row.try_get("rules_version")?,
            settled_at: row.try_get("settled_at")?,
        })
    })
    .fetch_optional(ex)
    .await
}
