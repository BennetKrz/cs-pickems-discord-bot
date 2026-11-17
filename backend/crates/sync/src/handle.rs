//! The handle other components use to talk to the sync scheduler: trigger
//! a structure sync of one event (follow, manual resync), search events
//! within the budget, and read the counters for the admin status view.
//! Only the sync worker calls external APIs; the handle goes through it.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use domain::EventId;
use serde::Serialize;
use sources::{CallKind, Capability, Sourced, SrcEvent};

use crate::budget::{Priority, Shortfall};
use crate::scheduler::{BUDGET_JOB, Core};

/// A cheap, clonable handle to the running scheduler.
#[derive(Clone)]
pub struct SyncHandle {
    core: Arc<Core>,
}

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub source: &'static str,
    pub event: Sourced<SrcEvent>,
}

/// Search results from every source that could answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// Sources that were not asked or failed, with the reason.
    pub skipped: Vec<(&'static str, String)>,
}

/// Counters of one source.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceStatus {
    pub source: &'static str,
    /// The budget caps; `None` for an unbudgeted source.
    pub budget_per_hour: Option<u32>,
    pub budget_per_minute: Option<u32>,
    /// Tokens left in the hourly bucket now.
    pub budget_left: Option<u32>,
    /// Requests sent in the current UTC clock hour.
    pub requests_this_hour: i64,
    pub hour_started_at: DateTime<Utc>,
    pub last_request_at: Option<DateTime<Utc>>,
    /// The quota the source last reported (e.g. `X-Rate-Limit-Remaining`).
    pub remaining_quota: Option<i64>,
}

/// State of one job of one source.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobStatus {
    pub job: String,
    pub source: String,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// When the scheduler plans to run it next, if it has work.
    pub next_run_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncStatus {
    pub now: DateTime<Utc>,
    pub sources: Vec<SourceStatus>,
    pub jobs: Vec<JobStatus>,
}

/// Lowercase, single spaces: the cache key of a query.
fn normalize(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl SyncHandle {
    pub(crate) fn new(core: Arc<Core>) -> Self {
        Self { core }
    }

    /// Runs the structure sync of `event` at once, for every source that
    /// knows it, followed or not (follow in T09, manual resync). Returns
    /// immediately; the scheduler picks it up on its next tick.
    pub fn sync_event_now(&self, event: EventId) {
        self.core
            .forced
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(event);
        self.core.wake.notify_one();
    }

    /// Searches events on every source with the search capability. Results
    /// are cached per source and normalized query. A search is the lowest
    /// priority: when the budget is short it is skipped, not queued.
    pub async fn search(&self, query: &str) -> SearchOutcome {
        let key = normalize(query);
        let mut out = SearchOutcome::default();
        if key.is_empty() {
            return out;
        }
        let now = self.core.clock.now();
        let ttl = self.core.config.search_cache;
        for (idx, src) in self.core.sources.iter().enumerate() {
            let id = src.adapter.id();
            if !src.adapter.supports(Capability::Search) {
                continue;
            }
            let cache_key = (id.to_owned(), key.clone());
            let cached = {
                let cache = self.core.search_cache.lock().await;
                cache
                    .get(&cache_key)
                    .filter(|(at, _)| *at + ttl > now)
                    .map(|(_, v)| v.clone())
            };
            let events = match cached {
                Some(v) => v,
                None => {
                    let cost = src.adapter.cost(CallKind::SearchEvents, 0);
                    match self.core.spend(idx, cost, Priority::Search, now).await {
                        Ok(Ok(())) => {}
                        Ok(Err(s)) => {
                            let why = match s {
                                Shortfall::Hour => "hourly budget",
                                Shortfall::Minute => "per-minute budget",
                            };
                            out.skipped.push((id, format!("{why} is short; try later")));
                            continue;
                        }
                        Err(e) => {
                            out.skipped.push((id, format!("database error: {e}")));
                            continue;
                        }
                    }
                    let res = src.adapter.search_events(&key).await;
                    if let Err(e) = self.core.note_quota(idx, now).await {
                        tracing::warn!("cannot store quota: {e}");
                    }
                    match res {
                        Ok(v) => {
                            self.core
                                .search_cache
                                .lock()
                                .await
                                .insert(cache_key, (now, v.clone()));
                            v
                        }
                        Err(e) => {
                            out.skipped.push((id, e.to_string()));
                            continue;
                        }
                    }
                }
            };
            out.hits.extend(
                events
                    .into_iter()
                    .map(|event| SearchHit { source: id, event }),
            );
        }
        out
    }

    /// Counters per source and state per job, for `/api/admin/status`.
    pub async fn status(&self) -> sqlx::Result<SyncStatus> {
        let now = self.core.clock.now();
        let mut sources = Vec::new();
        for src in &self.core.sources {
            let mut m = src.meter.lock().await;
            let limits = m.budget.as_ref().map(|b| b.limits());
            let budget_left = m.budget.as_mut().map(|b| b.hour_left(now));
            sources.push(SourceStatus {
                source: src.adapter.id(),
                budget_per_hour: limits.map(|l| l.per_hour),
                budget_per_minute: limits.map(|l| l.per_minute),
                budget_left,
                requests_this_hour: m.requests,
                hour_started_at: m.hour_start,
                last_request_at: m.last_request_at,
                remaining_quota: m.remaining,
            });
        }
        let next_runs = self
            .core
            .next_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let known: Vec<&str> = self.core.sources.iter().map(|s| s.adapter.id()).collect();
        let jobs = store::sync_state::list(&self.core.pool)
            .await?
            .into_iter()
            .filter(|s| s.job != BUDGET_JOB && known.contains(&s.source.as_str()))
            .map(|s| JobStatus {
                next_run_at: next_runs.get(&(s.job.clone(), s.source.clone())).copied(),
                job: s.job,
                source: s.source,
                last_run_at: s.last_run_at,
                last_success_at: s.last_success_at,
                last_error: s.last_error,
            })
            .collect();
        Ok(SyncStatus { now, sources, jobs })
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn queries_are_normalized() {
        assert_eq!(normalize("  BLAST   Rivals "), "blast rivals");
        assert_eq!(normalize("   "), "");
    }
}
