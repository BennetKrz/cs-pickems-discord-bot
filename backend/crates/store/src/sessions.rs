//! Server-side sessions of the Activity (T07).
//!
//! The bearer token itself is never stored, only its SHA-256 as lowercase
//! hex (`token_hash`). Hashing and token generation live in the `api`
//! crate; this module only stores rows.

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::{DbResult, Ts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// SHA-256 of the bearer token, lowercase hex.
    pub token_hash: String,
    pub user_id: String,
    /// The guild verified at sign-in.
    pub guild_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

fn from_row(row: SqliteRow) -> DbResult<Session> {
    Ok(Session {
        token_hash: row.try_get("token_hash")?,
        user_id: row.try_get("user_id")?,
        guild_id: row.try_get("guild_id")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
    })
}

/// Stores a new session. The user must exist.
pub async fn create(ex: impl SqliteExecutor<'_>, s: &Session) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, guild_id, created_at, expires_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&s.token_hash)
    .bind(&s.user_id)
    .bind(&s.guild_id)
    .bind(Ts(s.created_at))
    .bind(Ts(s.expires_at))
    .execute(ex)
    .await?;
    Ok(())
}

/// The session with this hash, if it exists and has not expired at `now`.
pub async fn get_valid(
    ex: impl SqliteExecutor<'_>,
    token_hash: &str,
    now: DateTime<Utc>,
) -> DbResult<Option<Session>> {
    sqlx::query(
        "SELECT token_hash, user_id, guild_id, created_at, expires_at FROM sessions \
         WHERE token_hash = ? AND expires_at > ?",
    )
    .bind(token_hash)
    .bind(Ts(now))
    .try_map(from_row)
    .fetch_optional(ex)
    .await
}

/// Deletes one session. Returns `false` if it did not exist.
pub async fn delete(ex: impl SqliteExecutor<'_>, token_hash: &str) -> DbResult<bool> {
    let res = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(ex)
        .await?;
    Ok(res.rows_affected() == 1)
}

/// Deletes every session that has expired at `now`; returns how many.
pub async fn delete_expired(ex: impl SqliteExecutor<'_>, now: DateTime<Utc>) -> DbResult<u64> {
    let res = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(Ts(now))
        .execute(ex)
        .await?;
    Ok(res.rows_affected())
}
