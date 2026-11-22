//! The Activity deep link (T08): *Pick now* on a bot post stores which
//! match the member wants to open; the Activity takes it once on start.
//! One pending view per member and guild; a newer click replaces it.

use chrono::{DateTime, Utc};
use domain::MatchId;
use sqlx::SqliteExecutor;

use crate::{DbResult, Ts};

/// Stores (or replaces) a member's pending view.
pub async fn put(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    user_id: &str,
    match_id: MatchId,
    expires_at: DateTime<Utc>,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO pending_views (guild_id, user_id, match_id, expires_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT (guild_id, user_id) DO UPDATE SET match_id = excluded.match_id, \
           expires_at = excluded.expires_at",
    )
    .bind(guild_id)
    .bind(user_id)
    .bind(match_id.0)
    .bind(Ts(expires_at))
    .execute(ex)
    .await?;
    Ok(())
}

/// Removes a member's pending view and returns its match if it had not
/// expired at `now`. A second call returns `None`.
pub async fn take(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    user_id: &str,
    now: DateTime<Utc>,
) -> DbResult<Option<MatchId>> {
    let row: Option<(i64, DateTime<Utc>)> = sqlx::query_as(
        "DELETE FROM pending_views WHERE guild_id = ? AND user_id = ? \
         RETURNING match_id, expires_at",
    )
    .bind(guild_id)
    .bind(user_id)
    .fetch_optional(ex)
    .await?;
    Ok(row.filter(|(_, exp)| *exp > now).map(|(m, _)| MatchId(m)))
}

/// Deletes every pending view that expired before `now`. Returns how many.
pub async fn purge_expired(ex: impl SqliteExecutor<'_>, now: DateTime<Utc>) -> DbResult<u64> {
    let res = sqlx::query("DELETE FROM pending_views WHERE expires_at <= ?")
        .bind(Ts(now))
        .execute(ex)
        .await?;
    Ok(res.rows_affected())
}
