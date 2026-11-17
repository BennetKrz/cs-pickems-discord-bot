//! Per-guild settings and followed events.

use chrono::{DateTime, Utc};
use domain::EventId;
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::types::Json;
use sqlx::{Row, SqliteExecutor};

use crate::{DbResult, Ts};

#[derive(Debug, Clone, PartialEq)]
pub struct GuildSettings {
    pub guild_id: String,
    pub announce_channel_id: Option<String>,
    pub admin_role_id: Option<String>,
    /// `None` means the default rules (T11).
    pub scoring_rules: Option<Value>,
    pub scoring_rules_version: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowedEvent {
    pub guild_id: String,
    pub event_id: EventId,
    /// Discord user ID of the admin who followed it.
    pub followed_by: String,
    pub followed_at: DateTime<Utc>,
}

fn settings_from_row(row: SqliteRow) -> DbResult<GuildSettings> {
    Ok(GuildSettings {
        guild_id: row.try_get("guild_id")?,
        announce_channel_id: row.try_get("announce_channel_id")?,
        admin_role_id: row.try_get("admin_role_id")?,
        scoring_rules: row
            .try_get::<Option<Json<Value>>, _>("scoring_rules")?
            .map(|j| j.0),
        scoring_rules_version: row.try_get("scoring_rules_version")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn followed_from_row(row: SqliteRow) -> DbResult<FollowedEvent> {
    Ok(FollowedEvent {
        guild_id: row.try_get("guild_id")?,
        event_id: EventId(row.try_get("event_id")?),
        followed_by: row.try_get("followed_by")?,
        followed_at: row.try_get("followed_at")?,
    })
}

pub async fn get_settings(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
) -> DbResult<Option<GuildSettings>> {
    sqlx::query(
        "SELECT guild_id, announce_channel_id, admin_role_id, scoring_rules, \
           scoring_rules_version, updated_at \
         FROM guild_settings WHERE guild_id = ?",
    )
    .bind(guild_id)
    .try_map(settings_from_row)
    .fetch_optional(ex)
    .await
}

/// Inserts or overwrites a guild's settings.
pub async fn upsert_settings(ex: impl SqliteExecutor<'_>, s: &GuildSettings) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, announce_channel_id, admin_role_id, \
           scoring_rules, scoring_rules_version, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT (guild_id) DO UPDATE SET \
           announce_channel_id = excluded.announce_channel_id, \
           admin_role_id = excluded.admin_role_id, \
           scoring_rules = excluded.scoring_rules, \
           scoring_rules_version = excluded.scoring_rules_version, \
           updated_at = excluded.updated_at",
    )
    .bind(&s.guild_id)
    .bind(&s.announce_channel_id)
    .bind(&s.admin_role_id)
    .bind(s.scoring_rules.as_ref().map(Json))
    .bind(s.scoring_rules_version)
    .bind(Ts(s.updated_at))
    .execute(ex)
    .await?;
    Ok(())
}

/// Follows an event. Returns `false` if it was already followed; the
/// original follower and time are kept.
pub async fn follow_event(ex: impl SqliteExecutor<'_>, f: &FollowedEvent) -> DbResult<bool> {
    let res = sqlx::query(
        "INSERT INTO followed_events (guild_id, event_id, followed_by, followed_at) \
         VALUES (?, ?, ?, ?) ON CONFLICT (guild_id, event_id) DO NOTHING",
    )
    .bind(&f.guild_id)
    .bind(f.event_id.0)
    .bind(&f.followed_by)
    .bind(Ts(f.followed_at))
    .execute(ex)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Unfollows an event. Returns `false` if it was not followed. Picks and
/// results are untouched.
pub async fn unfollow_event(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    event_id: EventId,
) -> DbResult<bool> {
    let res = sqlx::query("DELETE FROM followed_events WHERE guild_id = ? AND event_id = ?")
        .bind(guild_id)
        .bind(event_id.0)
        .execute(ex)
        .await?;
    Ok(res.rows_affected() == 1)
}

/// The events a guild follows, oldest follow first.
pub async fn list_followed(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
) -> DbResult<Vec<FollowedEvent>> {
    sqlx::query(
        "SELECT guild_id, event_id, followed_by, followed_at FROM followed_events \
         WHERE guild_id = ? ORDER BY followed_at, event_id",
    )
    .bind(guild_id)
    .try_map(followed_from_row)
    .fetch_all(ex)
    .await
}

/// Every event any guild follows, by ID. The sync scheduler (T06) keeps
/// exactly these fresh.
pub async fn followed_event_ids(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<EventId>> {
    sqlx::query_scalar("SELECT DISTINCT event_id FROM followed_events ORDER BY event_id")
        .fetch_all(ex)
        .await
        .map(|ids: Vec<i64>| ids.into_iter().map(EventId).collect())
}

pub async fn is_followed(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    event_id: EventId,
) -> DbResult<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM followed_events WHERE guild_id = ? AND event_id = ?)",
    )
    .bind(guild_id)
    .bind(event_id.0)
    .fetch_one(ex)
    .await
}
