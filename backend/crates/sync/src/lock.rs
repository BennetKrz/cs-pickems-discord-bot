//! The lock ticker (T06): every `lock_every` it locks the picks of matches
//! whose `lock_at` has passed or that are already live or finished (rules
//! in T10). It runs independently of the sync jobs and makes no external
//! calls. The pick API (T10) also checks `lock_at` itself, so a request
//! between two ticks is still rejected.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use domain::{Clock, MatchId};
use store::SqlitePool;
use tokio_util::sync::CancellationToken;

/// Locks every due match at `now` and returns the newly locked ones.
pub async fn lock_due(pool: &SqlitePool, now: DateTime<Utc>) -> sqlx::Result<Vec<MatchId>> {
    let locked = store::matches::lock_due(pool, now).await?;
    if !locked.is_empty() {
        tracing::info!(count = locked.len(), matches = ?locked, "picks locked");
    }
    Ok(locked)
}

/// Runs the lock ticker until `cancel` fires.
pub async fn run_lock_ticker(
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
    every: std::time::Duration,
    cancel: CancellationToken,
) {
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = interval.tick() => {
                if let Err(e) = lock_due(&pool, clock.now()).await {
                    tracing::warn!("lock ticker: {e}");
                }
            }
        }
    }
}
