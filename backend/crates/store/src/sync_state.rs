//! Restart-safe state of the sync scheduler, per job and source (T06).

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::{DbResult, Ts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncState {
    pub job: String,
    pub source: String,
    /// Opaque job-specific position, e.g. a page token.
    pub cursor: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// Requests made since `window_started_at`.
    pub request_count: i64,
    pub window_started_at: Option<DateTime<Utc>>,
    /// Last quota the source reported, if it reports one.
    pub remaining_quota: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

impl SyncState {
    /// A fresh state for a job that has never run.
    pub fn new(job: &str, source: &str, now: DateTime<Utc>) -> Self {
        Self {
            job: job.to_owned(),
            source: source.to_owned(),
            cursor: None,
            last_run_at: None,
            last_success_at: None,
            last_error: None,
            request_count: 0,
            window_started_at: None,
            remaining_quota: None,
            updated_at: now,
        }
    }
}

fn from_row(row: SqliteRow) -> DbResult<SyncState> {
    Ok(SyncState {
        job: row.try_get("job")?,
        source: row.try_get("source")?,
        cursor: row.try_get("cursor")?,
        last_run_at: row.try_get("last_run_at")?,
        last_success_at: row.try_get("last_success_at")?,
        last_error: row.try_get("last_error")?,
        request_count: row.try_get("request_count")?,
        window_started_at: row.try_get("window_started_at")?,
        remaining_quota: row.try_get("remaining_quota")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub async fn get(
    ex: impl SqliteExecutor<'_>,
    job: &str,
    source: &str,
) -> DbResult<Option<SyncState>> {
    sqlx::query(
        "SELECT job, source, cursor, last_run_at, last_success_at, last_error, request_count, \
           window_started_at, remaining_quota, updated_at \
         FROM sync_state WHERE job = ? AND source = ?",
    )
    .bind(job)
    .bind(source)
    .try_map(from_row)
    .fetch_optional(ex)
    .await
}

/// Inserts or overwrites the state of one job and source.
pub async fn put(ex: impl SqliteExecutor<'_>, s: &SyncState) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO sync_state (job, source, cursor, last_run_at, last_success_at, last_error, \
           request_count, window_started_at, remaining_quota, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (job, source) DO UPDATE SET \
           cursor = excluded.cursor, last_run_at = excluded.last_run_at, \
           last_success_at = excluded.last_success_at, last_error = excluded.last_error, \
           request_count = excluded.request_count, \
           window_started_at = excluded.window_started_at, \
           remaining_quota = excluded.remaining_quota, updated_at = excluded.updated_at",
    )
    .bind(&s.job)
    .bind(&s.source)
    .bind(&s.cursor)
    .bind(Ts::opt(s.last_run_at))
    .bind(Ts::opt(s.last_success_at))
    .bind(&s.last_error)
    .bind(s.request_count)
    .bind(Ts::opt(s.window_started_at))
    .bind(s.remaining_quota)
    .bind(Ts(s.updated_at))
    .execute(ex)
    .await?;
    Ok(())
}

/// Every stored state, for the admin status view.
pub async fn list(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<SyncState>> {
    sqlx::query(
        "SELECT job, source, cursor, last_run_at, last_success_at, last_error, request_count, \
           window_started_at, remaining_quota, updated_at \
         FROM sync_state ORDER BY job, source",
    )
    .try_map(from_row)
    .fetch_all(ex)
    .await
}
