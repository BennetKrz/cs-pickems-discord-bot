//! The sync scheduler (T06): decides which external calls to make, by match
//! state and within each source's budget, and feeds the results through the
//! reconciler.
//!
//! Jobs, per source, most urgent first:
//!
//! | Job | Selects | Call |
//! | --- | --- | --- |
//! | `live` | live matches | `fetch_live`, then `fetch_matches` for IDs it no longer lists |
//! | `imminent` | starting within 1 h, or overdue by at most 6 h | `fetch_matches` |
//! | `corrections` | finished matches 10 min and 2 h after their end | `fetch_matches` |
//! | `upcoming` | starting within 48 h | `fetch_matches` |
//! | `structure:<event>` | each followed, unfinished event | `fetch_event` + `fetch_event_matches` |
//!
//! Every job is one batched call for all its matches and is skipped when it
//! has none. Only followed events are synced; a manual resync
//! ([`SyncHandle::sync_event_now`]) runs the structure job of any event at
//! once. Job state lives in `sync_state`, so a restart neither repeats
//! fresh jobs nor catches up missed runs one by one: an overdue job runs
//! once, after a random startup delay. See
//! `docs/adr/0006-sync-scheduler-budget.md`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};

use chrono::{DateTime, Duration, DurationRound, Utc};
use domain::{Clock, EntityType, EventId, EventStatus, MatchFields, MatchStatus};
use sources::{CallKind, Capability, SourceAdapter, SourceError, Sourced, SrcEvent, SrcMatch};
use store::SqlitePool;
use store::sync_state::{self, SyncState};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

use crate::budget::{Budget, Priority, SavedBudget, Shortfall};
use crate::config::{BudgetLimits, SyncConfig};
use crate::handle::SyncHandle;
use crate::reconcile::{ReconcileError, Reconciler, Report};

/// The `sync_state` job holding a source's budget and request counters.
pub const BUDGET_JOB: &str = "budget";
pub const LIVE_JOB: &str = "live";
pub const IMMINENT_JOB: &str = "imminent";
pub const CORRECTIONS_JOB: &str = "corrections";
pub const UPCOMING_JOB: &str = "upcoming";
/// Prefix of the per-event structure jobs, `structure:<event ID>`.
pub const STRUCTURE_JOB_PREFIX: &str = "structure:";

/// Errors of one job.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Reconcile(#[from] ReconcileError),
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// One source the scheduler polls, with its budget. `None` means no budget
/// (a source that costs nothing, such as fixtures).
pub struct SourceEntry {
    pub adapter: Arc<dyn SourceAdapter>,
    pub budget: Option<BudgetLimits>,
    /// Only for [`SyncHandle::search`]: no job ever polls it. The dev
    /// fixture source is one, because the dev clock drives its sync (T05,
    /// T09).
    pub search_only: bool,
}

/// Request counters and budget of one source.
#[derive(Debug)]
pub(crate) struct Meter {
    pub budget: Option<Budget>,
    /// Start of the UTC clock hour `requests` counts.
    pub hour_start: DateTime<Utc>,
    pub requests: i64,
    pub remaining: Option<i64>,
    pub last_request_at: Option<DateTime<Utc>>,
}

pub(crate) struct Source {
    pub adapter: Arc<dyn SourceAdapter>,
    pub meter: Mutex<Meter>,
    pub search_only: bool,
}

/// State shared by the scheduler loop and its handles.
pub(crate) struct Core {
    pub pool: SqlitePool,
    pub reconciler: Arc<Reconciler>,
    pub clock: Arc<dyn Clock>,
    pub config: SyncConfig,
    pub sources: Vec<Source>,
    /// Events whose structure job should run at once.
    pub forced: StdMutex<BTreeSet<EventId>>,
    pub wake: Notify,
    /// Next planned run per `(job, source)`, for the status view.
    pub next_runs: StdMutex<BTreeMap<(String, String), DateTime<Utc>>>,
    pub search_cache: Mutex<SearchCache>,
}

/// Search results per `(source, normalized query)`, with their fetch time.
pub(crate) type SearchCache = HashMap<(String, String), (DateTime<Utc>, Vec<Sourced<SrcEvent>>)>;

fn lock<T>(m: &StdMutex<T>) -> MutexGuard<'_, T> {
    // Nothing panics while holding these locks with data half written.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn hour_of(t: DateTime<Utc>) -> DateTime<Utc> {
    t.duration_trunc(Duration::hours(1)).unwrap_or(t)
}

impl Core {
    pub fn source_id(&self, idx: usize) -> &'static str {
        self.sources[idx].adapter.id()
    }

    /// Charges `cost` requests to source `idx`, or says why not. Updates
    /// and persists the counters before the call is made.
    pub async fn spend(
        &self,
        idx: usize,
        cost: u32,
        priority: Priority,
        now: DateTime<Utc>,
    ) -> Result<Result<(), Shortfall>, sqlx::Error> {
        let src = &self.sources[idx];
        let mut m = src.meter.lock().await;
        if let Some(b) = m.budget.as_mut()
            && let Err(s) = b.try_spend(cost, priority, now)
        {
            return Ok(Err(s));
        }
        if cost > 0 {
            let hour = hour_of(now);
            if hour != m.hour_start {
                m.hour_start = hour;
                m.requests = 0;
            }
            m.requests += i64::from(cost);
            m.last_request_at = Some(now);
            self.save_meter(src.adapter.id(), &m, now).await?;
        }
        Ok(Ok(()))
    }

    /// Records the quota the source reported on its last response.
    pub async fn note_quota(&self, idx: usize, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
        let src = &self.sources[idx];
        let Some(q) = src.adapter.quota() else {
            return Ok(());
        };
        let mut m = src.meter.lock().await;
        let remaining = Some(i64::from(q.remaining));
        if m.remaining != remaining {
            m.remaining = remaining;
            self.save_meter(src.adapter.id(), &m, now).await?;
        }
        Ok(())
    }

    async fn save_meter(&self, source: &str, m: &Meter, now: DateTime<Utc>) -> sqlx::Result<()> {
        let st = SyncState {
            job: BUDGET_JOB.to_owned(),
            source: source.to_owned(),
            cursor: m
                .budget
                .as_ref()
                .map(|b| serde_json::to_string(&b.saved()).expect("serializable")),
            last_run_at: m.last_request_at,
            last_success_at: None,
            last_error: None,
            request_count: m.requests,
            window_started_at: Some(m.hour_start),
            remaining_quota: m.remaining,
            updated_at: now,
        };
        sync_state::put(&self.pool, &st).await
    }
}

/// Loads a source's meter from `sync_state`: the buckets are restored, not
/// refilled to full, so a restart cannot burst.
async fn load_meter(
    pool: &SqlitePool,
    source: &str,
    limits: Option<BudgetLimits>,
    now: DateTime<Utc>,
) -> sqlx::Result<Meter> {
    let st = sync_state::get(pool, BUDGET_JOB, source).await?;
    let saved: Option<SavedBudget> = st
        .as_ref()
        .and_then(|s| s.cursor.as_deref())
        .and_then(|c| serde_json::from_str(c).ok());
    let budget = limits.map(|l| match saved {
        Some(s) => Budget::restore(l, s, now),
        None => Budget::full(l, now),
    });
    let hour = hour_of(now);
    let (requests, remaining, last) = match &st {
        Some(s) if s.window_started_at == Some(hour) => {
            (s.request_count, s.remaining_quota, s.last_run_at)
        }
        Some(s) => (0, s.remaining_quota, s.last_run_at),
        None => (0, None, None),
    };
    Ok(Meter {
        budget,
        hour_start: hour,
        requests,
        remaining,
        last_request_at: last,
    })
}

/// Which job a match belongs to right now, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchJob {
    Live,
    Imminent,
    Upcoming,
}

/// Classifies a match by status and start time. Finished matches are
/// handled by the corrections job, everything else only by the structure
/// job.
pub fn classify(m: &MatchFields, now: DateTime<Utc>, cfg: &SyncConfig) -> Option<MatchJob> {
    match m.status {
        MatchStatus::Live => Some(MatchJob::Live),
        MatchStatus::Scheduled | MatchStatus::Postponed => {
            let s = m.scheduled_at?;
            if s - now <= cfg.imminent_window && now - s <= cfg.overdue_window {
                Some(MatchJob::Imminent)
            } else if s > now && s - now <= cfg.upcoming_window {
                Some(MatchJob::Upcoming)
            } else {
                None
            }
        }
        MatchStatus::Finished | MatchStatus::Canceled => None,
    }
}

/// When a finished match is due for correction re-checks: its end plus
/// each offset.
pub fn correction_times(m: &MatchFields, cfg: &SyncConfig) -> Vec<DateTime<Utc>> {
    if m.status != MatchStatus::Finished {
        return Vec::new();
    }
    let Some(end) = m.ended_at.or(m.started_at).or(m.scheduled_at) else {
        return Vec::new();
    };
    cfg.corrections_after.iter().map(|d| end + *d).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum JobKind {
    Live,
    Imminent,
    Corrections,
    Upcoming,
    Structure(EventId),
}

impl JobKind {
    fn name(&self) -> String {
        match self {
            JobKind::Live => LIVE_JOB.to_owned(),
            JobKind::Imminent => IMMINENT_JOB.to_owned(),
            JobKind::Corrections => CORRECTIONS_JOB.to_owned(),
            JobKind::Upcoming => UPCOMING_JOB.to_owned(),
            JobKind::Structure(e) => format!("{STRUCTURE_JOB_PREFIX}{}", e.0),
        }
    }

    fn priority(&self) -> Priority {
        match self {
            JobKind::Live => Priority::Live,
            JobKind::Imminent => Priority::Imminent,
            JobKind::Corrections => Priority::Corrections,
            JobKind::Upcoming => Priority::Upcoming,
            JobKind::Structure(_) => Priority::Structure,
        }
    }

    fn cadence(&self, cfg: &SyncConfig) -> Duration {
        match self {
            JobKind::Live => cfg.live_every,
            JobKind::Imminent => cfg.imminent_every,
            // Due times come from the matches themselves.
            JobKind::Corrections => Duration::zero(),
            JobKind::Upcoming => cfg.upcoming_every,
            JobKind::Structure(_) => cfg.structure_every,
        }
    }
}

/// One planned job: a kind, a source, and the external IDs to ask for.
#[derive(Debug)]
struct Planned {
    kind: JobKind,
    source: usize,
    /// Match IDs, or the event ID for a structure job.
    ids: Vec<String>,
    forced: bool,
}

type JobKey = (String, usize);

/// What one tick did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    /// Jobs that called a source.
    pub ran: u32,
    /// Jobs that waited for budget.
    pub deferred: u32,
    /// Jobs whose call failed.
    pub failed: u32,
    /// What the reconciler did with the results.
    pub reconciled: Report,
}

enum Outcome {
    Done,
    Deferred(Shortfall),
    Failed(SyncError),
}

pub struct Scheduler {
    core: Arc<Core>,
    started_at: DateTime<Utc>,
    due: HashMap<JobKey, DateTime<Utc>>,
    /// Per source: end of the window already checked for corrections.
    corrections_mark: HashMap<usize, DateTime<Utc>>,
    warned: HashSet<JobKey>,
    rng: fastrand::Rng,
}

/// Everything a scheduler needs.
pub struct SchedulerParts {
    pub reconciler: Arc<Reconciler>,
    pub clock: Arc<dyn Clock>,
    pub config: SyncConfig,
    pub sources: Vec<SourceEntry>,
    /// Seed of the startup jitter; `None` for a random one. Tests fix it.
    pub seed: Option<u64>,
}

impl Scheduler {
    /// Builds the scheduler and restores budgets from `sync_state`.
    pub async fn open(parts: SchedulerParts) -> sqlx::Result<(Self, SyncHandle)> {
        let pool = parts.reconciler.pool().clone();
        let now = parts.clock.now();
        let mut sources = Vec::new();
        for e in parts.sources {
            let meter = load_meter(&pool, e.adapter.id(), e.budget, now).await?;
            sources.push(Source {
                adapter: e.adapter,
                meter: Mutex::new(meter),
                search_only: e.search_only,
            });
        }
        let core = Arc::new(Core {
            pool,
            reconciler: parts.reconciler,
            clock: parts.clock,
            config: parts.config,
            sources,
            forced: StdMutex::default(),
            wake: Notify::new(),
            next_runs: StdMutex::default(),
            search_cache: Mutex::default(),
        });
        let rng = match parts.seed {
            Some(s) => fastrand::Rng::with_seed(s),
            None => fastrand::Rng::new(),
        };
        let handle = SyncHandle::new(core.clone());
        Ok((
            Self {
                core,
                started_at: now,
                due: HashMap::new(),
                corrections_mark: HashMap::new(),
                warned: HashSet::new(),
                rng,
            },
            handle,
        ))
    }

    /// Runs ticks every `tick_every` (or sooner when a handle asks) until
    /// `cancel` fires.
    pub async fn run(mut self, cancel: CancellationToken) {
        tracing::info!(
            sources = ?self.core.sources.iter().map(|s| s.adapter.id()).collect::<Vec<_>>(),
            "sync scheduler started"
        );
        let every = self
            .core
            .config
            .tick_every
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(10));
        let core = self.core.clone();
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                res = self.tick() => {
                    if let Err(e) = res {
                        tracing::warn!("sync tick failed: {e}");
                    }
                }
            }
            tokio::select! {
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(every) => {}
                () = core.wake.notified() => {}
            }
        }
        tracing::info!("sync scheduler stopped");
    }

    /// Looks for due jobs at the clock's current time and runs them in
    /// priority order, within each source's budget.
    pub async fn tick(&mut self) -> Result<TickReport, sqlx::Error> {
        let now = self.core.clock.now();
        let forced: BTreeSet<EventId> = std::mem::take(&mut *lock(&self.core.forced));
        let mut plan = self.plan(now, &forced).await?;
        plan.sort_by_key(|j| (j.kind.priority(), j.source));

        let mut report = TickReport::default();
        let mut blocked: HashMap<usize, Priority> = HashMap::new();
        let mut keep_forced = BTreeSet::new();
        for job in plan {
            let key = (job.kind.name(), job.source);
            if !job.forced && !self.is_due(&key, &job.kind, now).await? {
                continue;
            }
            if blocked
                .get(&job.source)
                .is_some_and(|p| *p < job.kind.priority())
            {
                // A more urgent job of this source is waiting for budget.
                report.deferred += 1;
                if let JobKind::Structure(e) = job.kind
                    && job.forced
                {
                    keep_forced.insert(e);
                }
                continue;
            }
            match self.run_job(&job, now, &mut report.reconciled).await? {
                Outcome::Done => {
                    report.ran += 1;
                    self.warned.remove(&key);
                    self.due
                        .insert(key, now + job.kind.cadence(&self.core.config));
                }
                Outcome::Deferred(shortfall) => {
                    report.deferred += 1;
                    blocked.insert(job.source, job.kind.priority());
                    if let JobKind::Structure(e) = job.kind
                        && job.forced
                    {
                        keep_forced.insert(e);
                    }
                    if self.warned.insert(key.clone()) {
                        tracing::warn!(
                            job = key.0,
                            source = self.core.source_id(job.source),
                            ?shortfall,
                            "request budget short; job waits"
                        );
                    }
                }
                Outcome::Failed(e) => {
                    report.ran += 1;
                    report.failed += 1;
                    let retry = match &e {
                        SyncError::Source(SourceError::RateLimited {
                            retry_after: Some(d),
                        }) => Duration::from_std(*d).unwrap_or(self.core.config.failure_backoff),
                        _ => self
                            .core
                            .config
                            .failure_backoff
                            .max(job.kind.cadence(&self.core.config)),
                    };
                    tracing::warn!(
                        job = key.0,
                        source = self.core.source_id(job.source),
                        "sync job failed: {e}"
                    );
                    self.due.insert(key, now + retry);
                }
            }
        }
        if !keep_forced.is_empty() {
            lock(&self.core.forced).extend(keep_forced);
        }
        self.publish_next_runs();
        Ok(report)
    }

    fn publish_next_runs(&self) {
        let mut runs = lock(&self.core.next_runs);
        runs.clear();
        for ((job, idx), at) in &self.due {
            runs.insert((job.clone(), self.core.source_id(*idx).to_owned()), *at);
        }
    }

    /// Whether a job is due. The first time a job is seen, its next run is
    /// derived from `sync_state`: a job that completed recently waits for
    /// its cadence; an overdue, interrupted or failed one runs once, after
    /// a random delay if the scheduler just started.
    async fn is_due(
        &mut self,
        key: &JobKey,
        kind: &JobKind,
        now: DateTime<Utc>,
    ) -> sqlx::Result<bool> {
        if !self.due.contains_key(key) {
            let cfg = &self.core.config;
            let cadence = kind.cadence(cfg);
            let st = sync_state::get(&self.core.pool, &key.0, self.core.source_id(key.1)).await?;
            let base = match st {
                Some(SyncState {
                    last_run_at: Some(run),
                    last_success_at: Some(ok),
                    ..
                }) if ok >= run => run + cadence,
                _ => now,
            };
            let mut first = base;
            if now - self.started_at < cfg.startup_jitter {
                let max = (cadence / 2).min(cfg.startup_jitter);
                let max_ms = u64::try_from(max.num_milliseconds()).unwrap_or(0);
                let jitter =
                    Duration::milliseconds(i64::try_from(self.rng.u64(0..=max_ms)).unwrap_or(0));
                first = first.max(now + jitter);
            }
            self.due.insert(key.clone(), first);
        }
        Ok(self.due[key] <= now)
    }

    /// Collects the jobs that have work, without looking at due times.
    async fn plan(
        &mut self,
        now: DateTime<Utc>,
        forced: &BTreeSet<EventId>,
    ) -> sqlx::Result<Vec<Planned>> {
        let pool = &self.core.pool;
        let cfg = &self.core.config;
        let followed: BTreeSet<EventId> = store::guilds::followed_event_ids(pool)
            .await?
            .into_iter()
            .collect();

        // Per source: the window of correction times to check.
        let mut marks = Vec::new();
        for idx in 0..self.core.sources.len() {
            let mark = match self.corrections_mark.get(&idx) {
                Some(m) => *m,
                None => {
                    let st =
                        sync_state::get(pool, CORRECTIONS_JOB, self.core.source_id(idx)).await?;
                    st.and_then(|s| s.cursor)
                        .and_then(|c| DateTime::parse_from_rfc3339(&c).ok())
                        .map(|t| t.with_timezone(&Utc))
                        .unwrap_or(now - cfg.last_correction())
                }
            };
            marks.push(mark);
        }

        let mut by_job: BTreeMap<(JobKind, usize), BTreeSet<String>> = BTreeMap::new();
        let mut plan = Vec::new();
        for event_id in followed.union(forced) {
            let Some(event) = store::events::get(pool, *event_id).await? else {
                continue;
            };
            let is_followed = followed.contains(event_id);
            let is_forced = forced.contains(event_id);
            let matches = if is_followed {
                store::matches::list_for_event(pool, *event_id).await?
            } else {
                Vec::new()
            };
            for (idx, &mark) in marks.iter().enumerate() {
                if self.core.sources[idx].search_only {
                    continue;
                }
                let source = self.core.source_id(idx);
                if is_forced || (is_followed && event.status != EventStatus::Finished) {
                    let refs = store::refs::refs_of(pool, EntityType::Event, event_id.0).await?;
                    if let Some(r) = refs.into_iter().find(|r| r.source == source) {
                        plan.push(Planned {
                            kind: JobKind::Structure(*event_id),
                            source: idx,
                            ids: vec![r.external_id],
                            forced: is_forced,
                        });
                    }
                }
                if matches.is_empty() {
                    continue;
                }
                let refs = store::refs::match_refs_for_event(pool, source, event_id.0).await?;
                let mut ext: HashMap<i64, Vec<String>> = HashMap::new();
                for (id, e) in refs {
                    ext.entry(id).or_default().push(e);
                }
                for m in &matches {
                    let Some(ids) = ext.get(&m.id.0) else {
                        continue;
                    };
                    let kind = match classify(&m.fields, now, cfg) {
                        Some(MatchJob::Live) => Some(JobKind::Live),
                        Some(MatchJob::Imminent) => Some(JobKind::Imminent),
                        Some(MatchJob::Upcoming) => Some(JobKind::Upcoming),
                        None => correction_times(&m.fields, cfg)
                            .into_iter()
                            .any(|t| mark < t && t <= now)
                            .then_some(JobKind::Corrections),
                    };
                    if let Some(kind) = kind {
                        by_job
                            .entry((kind, idx))
                            .or_default()
                            .extend(ids.iter().cloned());
                    }
                }
            }
        }

        // Nothing to re-check: the window up to now is done.
        for (idx, &mark) in marks.iter().enumerate() {
            let has = by_job.contains_key(&(JobKind::Corrections, idx));
            self.corrections_mark
                .insert(idx, if has { mark } else { now.max(mark) });
        }

        plan.extend(by_job.into_iter().map(|((kind, source), ids)| Planned {
            kind,
            source,
            ids: ids.into_iter().collect(),
            forced: false,
        }));
        Ok(plan)
    }

    async fn run_job(
        &mut self,
        job: &Planned,
        now: DateTime<Utc>,
        reconciled: &mut Report,
    ) -> sqlx::Result<Outcome> {
        let core = self.core.clone();
        let adapter = core.sources[job.source].adapter.clone();
        let priority = job.kind.priority();
        let cost = match job.kind {
            JobKind::Structure(_) => adapter
                .cost(CallKind::FetchEvent, 0)
                .saturating_add(adapter.cost(CallKind::FetchEventMatches, 0)),
            JobKind::Live if adapter.supports(Capability::Live) => {
                adapter.cost(CallKind::FetchLive, job.ids.len())
            }
            _ => adapter.cost(CallKind::FetchMatches, job.ids.len()),
        };
        if let Err(s) = core.spend(job.source, cost, priority, now).await? {
            return Ok(Outcome::Deferred(s));
        }

        let name = job.kind.name();
        let source = adapter.id();
        let mut st = sync_state::get(&core.pool, &name, source)
            .await?
            .unwrap_or_else(|| SyncState::new(&name, source, now));
        st.last_run_at = Some(now);
        st.updated_at = now;
        sync_state::put(&core.pool, &st).await?;

        let result = self.execute(job, now).await;
        core.note_quota(job.source, now).await?;
        let outcome = match result {
            Ok(rep) => {
                *reconciled += rep;
                st.last_success_at = Some(now);
                st.last_error = (rep.failed > 0)
                    .then(|| format!("{} record(s) could not be reconciled", rep.failed));
                if job.kind == JobKind::Corrections {
                    st.cursor = Some(now.to_rfc3339());
                    self.corrections_mark.insert(job.source, now);
                }
                Outcome::Done
            }
            Err(e) => {
                st.last_error = Some(e.to_string());
                Outcome::Failed(e)
            }
        };
        st.updated_at = now;
        sync_state::put(&core.pool, &st).await?;
        Ok(outcome)
    }

    /// Makes the calls of one job (already paid for) and reconciles the
    /// results.
    async fn execute(&self, job: &Planned, now: DateTime<Utc>) -> Result<Report, SyncError> {
        let core = &self.core;
        let adapter = &core.sources[job.source].adapter;
        let source = adapter.id();
        let rec = &core.reconciler;
        match job.kind {
            JobKind::Structure(_) => {
                let ext = &job.ids[0];
                let ev = adapter.fetch_event(ext).await?;
                let mut rep = rec.ingest_event(source, &ev, now).await?;
                let ms = adapter.fetch_event_matches(ext).await?;
                rep += rec.ingest_matches(source, &ms, now).await?;
                Ok(rep)
            }
            JobKind::Live if adapter.supports(Capability::Live) => {
                let mut ms = adapter.fetch_live(&job.ids).await?;
                // A match that just ended is no longer listed as live; fetch
                // it normally to learn its result.
                let got: HashSet<&str> = ms.iter().map(|m| m.dto.ext_id.as_str()).collect();
                let missing: Vec<String> = job
                    .ids
                    .iter()
                    .filter(|id| !got.contains(id.as_str()))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    let cost = adapter.cost(CallKind::FetchMatches, missing.len());
                    match core.spend(job.source, cost, Priority::Live, now).await? {
                        Ok(()) => ms.extend(adapter.fetch_matches(&missing).await?),
                        Err(s) => tracing::warn!(
                            source,
                            ?s,
                            "request budget short; ended live matches wait"
                        ),
                    }
                }
                Ok(rec.ingest_matches(source, &ms, now).await?)
            }
            _ => {
                let ms: Vec<Sourced<SrcMatch>> = adapter.fetch_matches(&job.ids).await?;
                Ok(rec.ingest_matches(source, &ms, now).await?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use domain::{EventId, MatchFormat};

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap()
    }

    fn at(status: MatchStatus, start_in: Duration) -> MatchFields {
        let mut f = MatchFields::scheduled(
            EventId(1),
            None,
            None,
            MatchFormat::Bo3,
            Some(t0() + start_in),
        );
        f.status = status;
        f
    }

    #[test]
    fn matches_move_between_jobs_by_state_and_time() {
        let cfg = SyncConfig::default();
        let c = |status, mins| classify(&at(status, Duration::minutes(mins)), t0(), &cfg);
        use MatchStatus::*;
        assert_eq!(c(Scheduled, 49 * 60), None, "beyond 48 h");
        assert_eq!(c(Scheduled, 48 * 60), Some(MatchJob::Upcoming));
        assert_eq!(c(Scheduled, 61), Some(MatchJob::Upcoming));
        assert_eq!(c(Scheduled, 60), Some(MatchJob::Imminent));
        assert_eq!(c(Postponed, 5), Some(MatchJob::Imminent));
        assert_eq!(c(Scheduled, -6 * 60), Some(MatchJob::Imminent), "overdue");
        assert_eq!(c(Scheduled, -6 * 60 - 1), None, "given up");
        assert_eq!(c(Live, -30), Some(MatchJob::Live));
        assert_eq!(c(Finished, -300), None);
        assert_eq!(c(Canceled, 10), None);

        let mut unscheduled = at(Scheduled, Duration::zero());
        unscheduled.scheduled_at = None;
        assert_eq!(classify(&unscheduled, t0(), &cfg), None);
    }

    #[test]
    fn corrections_follow_the_end_of_finished_matches() {
        let cfg = SyncConfig::default();
        let mut f = at(MatchStatus::Finished, Duration::hours(-3));
        assert_eq!(
            correction_times(&f, &cfg),
            vec![
                t0() - Duration::hours(3) + Duration::minutes(10),
                t0() - Duration::hours(1)
            ],
            "falls back to the scheduled time"
        );
        f.ended_at = Some(t0());
        assert_eq!(
            correction_times(&f, &cfg),
            vec![t0() + Duration::minutes(10), t0() + Duration::hours(2)]
        );
        f.status = MatchStatus::Canceled;
        assert!(correction_times(&f, &cfg).is_empty());
    }
}
