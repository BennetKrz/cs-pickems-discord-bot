//! Scoring rules versions, per-match rules pins and the reads settlement
//! and the leaderboard need (T11).
//!
//! `guild_settings` holds a guild's current rules and version;
//! `scoring_rule_versions` keeps every version, so a match is settled with
//! the version that was active when it locked. `NULL` rules mean the
//! default rules (`domain::scoring::ScoringRules::default`). Validating the
//! rules JSON is the caller's job. See `docs/adr/0011-scoring-and-settlement.md`.

use chrono::{DateTime, Utc};
use domain::MatchId;
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::types::Json;
use sqlx::{Acquire, Row, Sqlite, SqliteExecutor};

use crate::guilds::{self, GuildSettings};
use crate::{DbResult, Ts};

/// The valid-from time of a guild's original rules, when a first change
/// writes them to the history: the beginning of time.
const BEGINNING: &str = "1970-01-01T00:00:00.000Z";

/// One rules version of a guild.
#[derive(Debug, Clone, PartialEq)]
pub struct RulesVersion {
    pub version: i64,
    /// `None` means the default rules.
    pub rules: Option<Value>,
}

/// The rules a match is settled with in one guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchPin {
    pub guild_id: String,
    pub match_id: MatchId,
    pub rules_version: i64,
    /// When settlement first saw the match finished.
    pub finished_seen_at: Option<DateTime<Utc>>,
}

/// A guild's current rules: version 1 and the default rules if the guild
/// has no settings yet.
pub async fn current_rules(ex: impl SqliteExecutor<'_>, guild_id: &str) -> DbResult<RulesVersion> {
    Ok(match guilds::get_settings(ex, guild_id).await? {
        Some(s) => RulesVersion {
            version: s.scoring_rules_version,
            rules: s.scoring_rules,
        },
        None => RulesVersion {
            version: 1,
            rules: None,
        },
    })
}

/// Makes `rules` the guild's current rules as a new version, valid from
/// `now`, and returns it. On the first change the previous version is
/// written to the history too, valid since the beginning of time.
pub async fn set_rules<'a, A>(
    conn: A,
    guild_id: &str,
    rules: &Value,
    now: DateTime<Utc>,
) -> DbResult<i64>
where
    A: Acquire<'a, Database = Sqlite>,
{
    let mut tx = conn.begin().await?;
    let settings = guilds::get_settings(&mut *tx, guild_id).await?;
    let current = current_rules(&mut *tx, guild_id).await?;
    let has_history: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM scoring_rule_versions WHERE guild_id = ?)",
    )
    .bind(guild_id)
    .fetch_one(&mut *tx)
    .await?;
    if !has_history {
        insert_version(&mut *tx, guild_id, &current, BEGINNING).await?;
    }
    let next = RulesVersion {
        version: current.version + 1,
        rules: Some(rules.clone()),
    };
    insert_version(&mut *tx, guild_id, &next, &crate::format_ts(&now)).await?;
    let settings = match settings {
        Some(s) => GuildSettings {
            scoring_rules: next.rules.clone(),
            scoring_rules_version: next.version,
            updated_at: now,
            ..s
        },
        None => GuildSettings {
            scoring_rules: next.rules.clone(),
            scoring_rules_version: next.version,
            ..GuildSettings::new(guild_id, now)
        },
    };
    guilds::upsert_settings(&mut *tx, &settings).await?;
    tx.commit().await?;
    Ok(next.version)
}

async fn insert_version(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    v: &RulesVersion,
    valid_from: &str,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO scoring_rule_versions (guild_id, version, rules, valid_from) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(guild_id)
    .bind(v.version)
    .bind(v.rules.as_ref().map(Json))
    .bind(valid_from)
    .execute(ex)
    .await?;
    Ok(())
}

/// The rules version that was active in a guild at `at`: the newest one
/// valid from `at` or earlier. A guild that never changed its rules has
/// only its current version (1 without settings).
pub async fn version_at(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    at: DateTime<Utc>,
) -> DbResult<i64> {
    sqlx::query_scalar(
        "SELECT coalesce( \
           (SELECT max(version) FROM scoring_rule_versions \
            WHERE guild_id = ? AND valid_from <= ?), \
           (SELECT scoring_rules_version FROM guild_settings WHERE guild_id = ?), \
           1)",
    )
    .bind(guild_id)
    .bind(Ts(at))
    .bind(guild_id)
    .fetch_one(ex)
    .await
}

/// A guild's rules of one version: from the history, else the current
/// rules if the version is current. `None` if the version is unknown.
pub async fn rules_of(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    version: i64,
) -> DbResult<Option<RulesVersion>> {
    // Sources in order of preference; the first row wins.
    let row: Option<Option<Json<Value>>> = sqlx::query_scalar(
        "SELECT rules FROM ( \
           SELECT rules, 1 AS pref FROM scoring_rule_versions \
             WHERE guild_id = ?1 AND version = ?2 \
           UNION ALL \
           SELECT scoring_rules, 2 FROM guild_settings \
             WHERE guild_id = ?1 AND scoring_rules_version = ?2 \
           UNION ALL \
           SELECT NULL, 3 WHERE ?2 = 1 \
             AND NOT EXISTS (SELECT 1 FROM guild_settings WHERE guild_id = ?1) \
         ) ORDER BY pref LIMIT 1",
    )
    .bind(guild_id)
    .bind(version)
    .fetch_optional(ex)
    .await?;
    Ok(row.map(|rules| RulesVersion {
        version,
        rules: rules.map(|j| j.0),
    }))
}

/// Every rules version a guild has had, oldest first, with its valid-from
/// time. Empty if the guild never changed its rules.
pub async fn history(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
) -> DbResult<Vec<(RulesVersion, DateTime<Utc>)>> {
    sqlx::query(
        "SELECT version, rules, valid_from FROM scoring_rule_versions \
         WHERE guild_id = ? ORDER BY version",
    )
    .bind(guild_id)
    .try_map(|row: SqliteRow| {
        Ok((
            RulesVersion {
                version: row.try_get("version")?,
                rules: row.try_get::<Option<Json<Value>>, _>("rules")?.map(|j| j.0),
            },
            row.try_get("valid_from")?,
        ))
    })
    .fetch_all(ex)
    .await
}

// ---- pins ---------------------------------------------------------------------

pub async fn get_pin(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    match_id: MatchId,
) -> DbResult<Option<MatchPin>> {
    sqlx::query(
        "SELECT guild_id, match_id, rules_version, finished_seen_at FROM match_scoring \
         WHERE guild_id = ? AND match_id = ?",
    )
    .bind(guild_id)
    .bind(match_id.0)
    .try_map(|row: SqliteRow| {
        Ok(MatchPin {
            guild_id: row.try_get("guild_id")?,
            match_id: MatchId(row.try_get("match_id")?),
            rules_version: row.try_get("rules_version")?,
            finished_seen_at: row.try_get("finished_seen_at")?,
        })
    })
    .fetch_optional(ex)
    .await
}

/// Inserts or replaces a pin.
pub async fn put_pin(ex: impl SqliteExecutor<'_>, p: &MatchPin) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO match_scoring (guild_id, match_id, rules_version, finished_seen_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT (guild_id, match_id) DO UPDATE SET \
           rules_version = excluded.rules_version, \
           finished_seen_at = excluded.finished_seen_at",
    )
    .bind(&p.guild_id)
    .bind(p.match_id.0)
    .bind(p.rules_version)
    .bind(Ts::opt(p.finished_seen_at))
    .execute(ex)
    .await?;
    Ok(())
}

/// Deletes the pins of a match in every guild, e.g. after a reopen.
pub async fn delete_pins_for_match(
    ex: impl SqliteExecutor<'_>,
    match_id: MatchId,
) -> DbResult<u64> {
    let res = sqlx::query("DELETE FROM match_scoring WHERE match_id = ?")
        .bind(match_id.0)
        .execute(ex)
        .await?;
    Ok(res.rows_affected())
}

// ---- reads --------------------------------------------------------------------

/// A member's points in a guild, summed over `pick_results`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTotal {
    pub user_id: String,
    pub points: i64,
}

/// Points per member of a guild, most points first, then by user ID. Only
/// members with at least one settled pick appear. The leaderboard (T14)
/// builds on this; it reads `pick_results` only.
pub async fn totals_for_guild(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
) -> DbResult<Vec<UserTotal>> {
    sqlx::query(
        "SELECT p.user_id, sum(r.points) AS points \
         FROM pick_results r JOIN picks p ON p.id = r.pick_id \
         WHERE p.guild_id = ? \
         GROUP BY p.user_id ORDER BY points DESC, p.user_id",
    )
    .bind(guild_id)
    .try_map(|row: SqliteRow| {
        Ok(UserTotal {
            user_id: row.try_get("user_id")?,
            points: row.try_get("points")?,
        })
    })
    .fetch_all(ex)
    .await
}

/// Matches whose results are missing, incomplete or out of step with the
/// match: picks without a result on a live, finished, canceled or voided
/// match; provisional results; parts still pending; void results on a
/// match that is no longer void (and the reverse); results on a match that
/// is open again. Settlement's periodic and catch-up sweep.
pub async fn matches_to_settle(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<MatchId>> {
    sqlx::query_scalar(
        "SELECT DISTINCT m.id FROM matches m \
           JOIN picks p ON p.match_id = m.id \
           LEFT JOIN pick_results r ON r.pick_id = p.id \
         WHERE CASE \
           WHEN m.voided = 1 OR m.status = 'canceled' \
             THEN r.pick_id IS NULL OR r.state <> 'void' \
           WHEN m.status IN ('live', 'finished') \
             THEN r.pick_id IS NULL OR r.state <> 'final' OR r.pending_parts > 0 \
           ELSE r.pick_id IS NOT NULL \
         END \
         ORDER BY m.id",
    )
    .fetch_all(ex)
    .await
    .map(|ids: Vec<i64>| ids.into_iter().map(MatchId).collect())
}

/// Every match that has at least one pick, for `app rescore --all`.
pub async fn matches_with_picks(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<MatchId>> {
    sqlx::query_scalar("SELECT DISTINCT match_id FROM picks ORDER BY match_id")
        .fetch_all(ex)
        .await
        .map(|ids: Vec<i64>| ids.into_iter().map(MatchId).collect())
}
