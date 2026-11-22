//! Reads for the server leaderboard (T14). [`load`] is the one query the
//! Activity (`GET /api/leaderboard`) and the bot's `/leaderboard` share: it
//! reads `pick_results` only and ranks with `domain::leaderboard`. See
//! `docs/adr/0016-leaderboard.md`.

use std::collections::HashMap;

use domain::EventId;
use domain::leaderboard::{self as rules, Board, ScoredPick};
use domain::scoring::Breakdown;
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::types::Json;
use sqlx::{Acquire, Row, Sqlite, SqliteExecutor};

use crate::DbResult;
use crate::guilds::{self, DEFAULT_TIMEZONE};
use crate::row::enum_col;

/// The settled picks of a guild, optionally of one event, with the start
/// (else the scheduled time) of their match.
pub async fn scored_picks(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    event_id: Option<EventId>,
) -> DbResult<Vec<ScoredPick>> {
    sqlx::query(
        "SELECT p.user_id, r.points, r.state, r.breakdown, \
           coalesce(m.started_at, m.scheduled_at) AS match_at \
         FROM pick_results r \
           JOIN picks p ON p.id = r.pick_id \
           JOIN matches m ON m.id = p.match_id \
         WHERE p.guild_id = ?1 AND (?2 IS NULL OR m.event_id = ?2) \
         ORDER BY r.pick_id",
    )
    .bind(guild_id)
    .bind(event_id.map(|e| e.0))
    .try_map(|row: SqliteRow| {
        let raw = row.try_get::<Json<Value>, _>("breakdown")?.0;
        // Settlement always writes a full breakdown. Should one not parse,
        // its points still count, only its hits don't: one bad row must not
        // take the whole board down.
        let breakdown = serde_json::from_value(raw).unwrap_or_else(|e| {
            tracing::warn!("a pick result has an unreadable breakdown: {e}");
            Breakdown {
                winner: None,
                series: None,
                maps: Vec::new(),
            }
        });
        Ok(ScoredPick {
            user_id: row.try_get("user_id")?,
            points: row.try_get("points")?,
            state: enum_col(&row, "state")?,
            breakdown,
            match_at: row.try_get("match_at")?,
        })
    })
    .fetch_all(ex)
    .await
}

/// How a member is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberInfo {
    pub user_id: String,
    /// Server nickname, else global name, else username.
    pub display_name: String,
    pub avatar_hash: Option<String>,
}

/// Names and avatars of users in a guild, by user ID. Users without a
/// stored profile are missing.
pub async fn members(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    user_ids: &[String],
) -> DbResult<Vec<MemberInfo>> {
    sqlx::query(
        "SELECT u.discord_user_id, u.avatar_hash, \
           coalesce(nullif(trim(gm.nick), ''), nullif(trim(u.global_name), ''), u.username) \
             AS display_name \
         FROM users u \
         LEFT JOIN guild_members gm ON gm.guild_id = ?1 AND gm.user_id = u.discord_user_id \
         WHERE u.discord_user_id IN (SELECT value FROM json_each(?2)) \
         ORDER BY u.discord_user_id",
    )
    .bind(guild_id)
    .bind(Json(user_ids))
    .try_map(|row: SqliteRow| {
        Ok(MemberInfo {
            user_id: row.try_get("discord_user_id")?,
            display_name: row.try_get("display_name")?,
            avatar_hash: row.try_get("avatar_hash")?,
        })
    })
    .fetch_all(ex)
    .await
}

/// An event the leaderboard can be filtered by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterEvent {
    pub id: EventId,
    pub name: String,
}

/// The events a guild follows, for the filter: the latest start first
/// (events without a start last), then by name.
pub async fn filter_events(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
) -> DbResult<Vec<FilterEvent>> {
    sqlx::query(
        "SELECT e.id, e.name FROM followed_events fe JOIN events e ON e.id = fe.event_id \
         WHERE fe.guild_id = ? \
         ORDER BY e.begin_at IS NULL, e.begin_at DESC, e.name, e.id",
    )
    .bind(guild_id)
    .try_map(|row: SqliteRow| {
        Ok(FilterEvent {
            id: EventId(row.try_get("id")?),
            name: row.try_get("name")?,
        })
    })
    .fetch_all(ex)
    .await
}

/// The ranked leaderboard with names and avatars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaderboard {
    pub board: Board,
    /// By user ID; a member without a stored profile is missing.
    pub members: HashMap<String, MemberInfo>,
}

impl Leaderboard {
    /// A member's display name, else the user ID.
    pub fn name_of<'a>(&'a self, user_id: &'a str) -> &'a str {
        self.members
            .get(user_id)
            .map_or(user_id, |m| m.display_name.as_str())
    }
}

/// The leaderboard of a guild, all time or for one event: ranked by the
/// tie-break, with the rank before the latest match day in the guild's time
/// zone. One read transaction.
pub async fn load<'a, A>(
    conn: A,
    guild_id: &str,
    event_id: Option<EventId>,
) -> DbResult<Leaderboard>
where
    A: Acquire<'a, Database = Sqlite>,
{
    let mut tx = conn.begin().await?;
    let tz_name = guilds::get_settings(&mut *tx, guild_id)
        .await?
        .map_or_else(|| DEFAULT_TIMEZONE.to_owned(), |s| s.timezone);
    let tz = domain::localtime::parse_tz(&tz_name).unwrap_or_else(|| {
        tracing::warn!(guild = %guild_id, timezone = %tz_name, "invalid time zone, using the default");
        // `DEFAULT_TIMEZONE`.
        chrono_tz::Europe::Berlin
    });
    let picks = scored_picks(&mut *tx, guild_id, event_id).await?;
    let mut ids: Vec<String> = picks.iter().map(|p| p.user_id.clone()).collect();
    ids.sort_unstable();
    ids.dedup();
    let members: HashMap<String, MemberInfo> = members(&mut *tx, guild_id, &ids)
        .await?
        .into_iter()
        .map(|m| (m.user_id.clone(), m))
        .collect();
    tx.commit().await?;

    let names: HashMap<String, String> = members
        .iter()
        .map(|(id, m)| (id.clone(), m.display_name.clone()))
        .collect();
    let board = rules::build(&picks, tz, &names);
    Ok(Leaderboard { board, members })
}
