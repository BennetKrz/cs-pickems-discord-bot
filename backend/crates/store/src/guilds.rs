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
    /// Local time of the daily digest, `HH:MM` (T08).
    pub digest_time: String,
    /// IANA time zone of `digest_time` (T08).
    pub timezone: String,
    /// When the announce channel was set; results of matches that finished
    /// earlier are never announced (T08).
    pub announce_since: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Default local time of the daily digest (T08).
pub const DEFAULT_DIGEST_TIME: &str = "10:00";
/// Default time zone of the daily digest (T08).
pub const DEFAULT_TIMEZONE: &str = "Europe/Berlin";

impl GuildSettings {
    /// A guild's settings before anyone changed them.
    pub fn new(guild_id: &str, now: DateTime<Utc>) -> Self {
        Self {
            guild_id: guild_id.to_owned(),
            announce_channel_id: None,
            admin_role_id: None,
            scoring_rules: None,
            scoring_rules_version: 1,
            digest_time: DEFAULT_DIGEST_TIME.to_owned(),
            timezone: DEFAULT_TIMEZONE.to_owned(),
            announce_since: None,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowedEvent {
    pub guild_id: String,
    pub event_id: EventId,
    /// Discord user ID of the admin who followed it.
    pub followed_by: String,
    pub followed_at: DateTime<Utc>,
}

macro_rules! settings_columns {
    () => {
        "guild_id, announce_channel_id, admin_role_id, scoring_rules, \
     scoring_rules_version, digest_time, timezone, announce_since, updated_at"
    };
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
        digest_time: row.try_get("digest_time")?,
        timezone: row.try_get("timezone")?,
        announce_since: row.try_get("announce_since")?,
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
    sqlx::query(concat!(
        "SELECT ",
        settings_columns!(),
        " FROM guild_settings WHERE guild_id = ?"
    ))
    .bind(guild_id)
    .try_map(settings_from_row)
    .fetch_optional(ex)
    .await
}

/// Every guild with an announce channel (T08), by guild ID.
pub async fn with_announce_channel(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<GuildSettings>> {
    sqlx::query(concat!(
        "SELECT ",
        settings_columns!(),
        " FROM guild_settings WHERE announce_channel_id IS NOT NULL ORDER BY guild_id"
    ))
    .try_map(settings_from_row)
    .fetch_all(ex)
    .await
}

/// Inserts or overwrites a guild's settings.
pub async fn upsert_settings(ex: impl SqliteExecutor<'_>, s: &GuildSettings) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, announce_channel_id, admin_role_id, \
           scoring_rules, scoring_rules_version, digest_time, timezone, announce_since, \
           updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (guild_id) DO UPDATE SET \
           announce_channel_id = excluded.announce_channel_id, \
           admin_role_id = excluded.admin_role_id, \
           scoring_rules = excluded.scoring_rules, \
           scoring_rules_version = excluded.scoring_rules_version, \
           digest_time = excluded.digest_time, \
           timezone = excluded.timezone, \
           announce_since = excluded.announce_since, \
           updated_at = excluded.updated_at",
    )
    .bind(&s.guild_id)
    .bind(&s.announce_channel_id)
    .bind(&s.admin_role_id)
    .bind(s.scoring_rules.as_ref().map(Json))
    .bind(s.scoring_rules_version)
    .bind(&s.digest_time)
    .bind(&s.timezone)
    .bind(Ts::opt(s.announce_since))
    .bind(Ts(s.updated_at))
    .execute(ex)
    .await?;
    Ok(())
}

/// Sets the announce channel (T08) and restarts `announce_since`, so only
/// results from now on are announced there. Other settings are kept.
pub async fn set_announce_channel(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    channel_id: &str,
    now: DateTime<Utc>,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, announce_channel_id, announce_since, updated_at) \
         VALUES (?1, ?2, ?3, ?3) \
         ON CONFLICT (guild_id) DO UPDATE SET \
           announce_channel_id = excluded.announce_channel_id, \
           announce_since = excluded.announce_since, updated_at = excluded.updated_at",
    )
    .bind(guild_id)
    .bind(channel_id)
    .bind(Ts(now))
    .execute(ex)
    .await?;
    Ok(())
}

/// Sets (or with `None` clears) the pick'em admin role (T07, T08). Other
/// settings are kept.
pub async fn set_admin_role(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    role_id: Option<&str>,
    now: DateTime<Utc>,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, admin_role_id, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT (guild_id) DO UPDATE SET admin_role_id = excluded.admin_role_id, \
           updated_at = excluded.updated_at",
    )
    .bind(guild_id)
    .bind(role_id)
    .bind(Ts(now))
    .execute(ex)
    .await?;
    Ok(())
}

/// Sets the daily digest's local time (`HH:MM`) and IANA time zone (T08).
/// The caller validates both. Other settings are kept.
pub async fn set_digest_schedule(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    digest_time: &str,
    timezone: &str,
    now: DateTime<Utc>,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, digest_time, timezone, updated_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (guild_id) DO UPDATE SET digest_time = excluded.digest_time, \
           timezone = excluded.timezone, updated_at = excluded.updated_at",
    )
    .bind(guild_id)
    .bind(digest_time)
    .bind(timezone)
    .bind(Ts(now))
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

/// One followed event with what `/event list` shows (T09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowedOverview {
    pub event: domain::Event,
    pub followed_by: String,
    pub followed_at: DateTime<Utc>,
    /// Matches of the event, of any state.
    pub matches: i64,
    /// Matches being played now (T13).
    pub live: i64,
    /// Picks the guild made on them.
    pub picks: i64,
}

/// The events a guild follows with match and pick counts, by start (events
/// without a start last), then name.
pub async fn followed_overview(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
) -> DbResult<Vec<FollowedOverview>> {
    sqlx::query(
        "SELECT e.id, e.name, e.tier, e.begin_at, e.end_at, e.status, \
           fe.followed_by, fe.followed_at, \
           (SELECT COUNT(*) FROM matches m WHERE m.event_id = e.id) AS match_count, \
           (SELECT COUNT(*) FROM matches m WHERE m.event_id = e.id AND m.status = 'live') \
             AS live_count, \
           (SELECT COUNT(*) FROM picks p JOIN matches m ON m.id = p.match_id \
             WHERE m.event_id = e.id AND p.guild_id = ?1) AS pick_count \
         FROM followed_events fe JOIN events e ON e.id = fe.event_id \
         WHERE fe.guild_id = ?1 \
         ORDER BY e.begin_at IS NULL, e.begin_at, e.name, e.id",
    )
    .bind(guild_id)
    .try_map(|row: SqliteRow| {
        Ok(FollowedOverview {
            event: domain::Event {
                id: EventId(row.try_get("id")?),
                name: row.try_get("name")?,
                tier: crate::row::enum_col(&row, "tier")?,
                begin_at: row.try_get("begin_at")?,
                end_at: row.try_get("end_at")?,
                status: crate::row::enum_col(&row, "status")?,
            },
            followed_by: row.try_get("followed_by")?,
            followed_at: row.try_get("followed_at")?,
            matches: row.try_get("match_count")?,
            live: row.try_get("live_count")?,
            picks: row.try_get("pick_count")?,
        })
    })
    .fetch_all(ex)
    .await
}
