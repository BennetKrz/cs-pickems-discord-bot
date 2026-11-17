//! Shared helpers for the scheduler tests: fixture scenarios generated per
//! test, a source that serves several of them, prices calls like PandaScore
//! and records every request, and a scheduler on a fake clock.

// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{Clock, EntityType, EventId, FakeClock};
use serde_json::json;
use sources::fixture::{FixtureAdapter, Scenario};
use sources::{
    CallKind, Capability, RateLimits, SourceAdapter, SourceError, SourceResult, Sourced, SrcEvent,
    SrcMatch,
};
use store::SqlitePool;
use store::guilds::FollowedEvent;
use sync::{
    BudgetLimits, EventBus, Precedence, Reconciler, Scheduler, SchedulerParts, SourceEntry,
    SyncConfig, SyncHandle,
};
use tokio::sync::Notify;

/// The simulated source's name.
pub const SIM: &str = "sim";

pub const GUILD: &str = "100000000000000001";
pub const ADMIN: &str = "200000000000000001";

/// The default budget: 50% of PandaScore's free plan.
pub const BUDGET: BudgetLimits = BudgetLimits {
    per_hour: 500,
    per_minute: 30,
};

/// Midnight of the simulated day.
pub fn day() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap()
}

/// A scenario with one Bo3 per start time (minutes after [`day`]), each
/// between two teams of its own, decided 2:1 after three 45-minute maps
/// (135 minutes).
pub fn scenario(event: &str, starts: &[i64]) -> Scenario {
    let mut teams = Vec::new();
    let mut matches = Vec::new();
    for (i, start) in starts.iter().enumerate() {
        let (a, b) = (format!("{event}-t{i}a"), format!("{event}-t{i}b"));
        teams.push(json!({ "id": a, "name": format!("Team {a}") }));
        teams.push(json!({ "id": b, "name": format!("Team {b}") }));
        matches.push(json!({
            "id": format!("{event}-m{i}"), "stage": "main", "format": "bo3", "start": start,
            "team_a": a, "team_b": b,
            "maps": [
                { "name": "Mirage", "rounds": [13, 5] },
                { "name": "Nuke", "rounds": [7, 13] },
                { "name": "Inferno", "rounds": [13, 10] }
            ]
        }));
    }
    let s = json!({
        "event": {
            "id": event, "name": format!("Event {event}"), "tier": "a",
            "stages": [{ "id": "main", "name": "Main Stage", "kind": "swiss", "position": 0 }]
        },
        "teams": teams,
        "matches": matches,
    });
    Scenario::from_json(&s.to_string()).unwrap()
}

/// Length of every generated match.
pub const MATCH_MINUTES: i64 = 135;

/// One request the scheduler made.
#[derive(Debug, Clone)]
pub struct Call {
    pub at: DateTime<Utc>,
    pub kind: CallKind,
    pub ids: Vec<String>,
    pub cost: u32,
}

/// Several fixture events behind one source, priced like PandaScore.
pub struct SimSource {
    events: BTreeMap<String, FixtureAdapter>,
    clock: Arc<FakeClock>,
    pub calls: Mutex<Vec<Call>>,
    /// The call with this index (0-based) never returns; `reached` fires
    /// when it starts. Simulates a process killed mid-sync.
    hang: Mutex<Option<(usize, Arc<Notify>)>>,
}

impl SimSource {
    pub fn new(scenarios: Vec<Scenario>, clock: Arc<FakeClock>) -> Self {
        let events = scenarios
            .into_iter()
            .map(|s| {
                (
                    s.event.id.clone(),
                    FixtureAdapter::new(s, day(), clock.clone()),
                )
            })
            .collect();
        Self {
            events,
            clock,
            calls: Mutex::default(),
            hang: Mutex::default(),
        }
    }

    pub fn fixture(&self, event: &str) -> &FixtureAdapter {
        &self.events[event]
    }

    /// Makes call number `index` (counted from all calls so far) hang.
    pub fn hang_on(&self, index: usize) -> Arc<Notify> {
        let n = Arc::new(Notify::new());
        *self.hang.lock().unwrap() = Some((index, n.clone()));
        n
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    async fn record(&self, kind: CallKind, ids: Vec<String>) {
        let cost = self.cost(kind, ids.len());
        let index = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(Call {
                at: self.clock.now(),
                kind,
                ids,
                cost,
            });
            calls.len() - 1
        };
        let hang = self.hang.lock().unwrap().clone();
        if let Some((i, reached)) = hang
            && i == index
        {
            reached.notify_one();
            std::future::pending::<()>().await;
        }
    }

    fn matches_by_id(&self, ext_ids: &[String], live_only: bool) -> Vec<Sourced<SrcMatch>> {
        let mut out = Vec::new();
        for f in self.events.values() {
            for m in f.matches() {
                if ext_ids.contains(&m.ext_id)
                    && (!live_only || m.status == Some(domain::MatchStatus::Live))
                {
                    out.push(Sourced::new(m, json!({ "sim": true })));
                }
            }
        }
        out
    }
}

#[async_trait]
impl SourceAdapter for SimSource {
    fn id(&self) -> &'static str {
        SIM
    }

    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::Schedule,
            Capability::Results,
            Capability::Brackets,
            Capability::Live,
            Capability::Search,
        ]
    }

    fn limits(&self) -> RateLimits {
        RateLimits {
            per_minute: 60,
            per_hour: 1000,
        }
    }

    /// PandaScore's estimates (ADR 0001): 1 per search or event, 3 per
    /// event match list, one per 100 IDs.
    fn cost(&self, call: CallKind, items: usize) -> u32 {
        match call {
            CallKind::SearchEvents | CallKind::FetchEvent => 1,
            CallKind::FetchEventMatches => 3,
            CallKind::FetchMatches | CallKind::FetchLive => {
                u32::try_from(items.div_ceil(100)).unwrap()
            }
        }
    }

    async fn search_events(&self, query: &str) -> SourceResult<Vec<Sourced<SrcEvent>>> {
        self.record(CallKind::SearchEvents, Vec::new()).await;
        let mut out = Vec::new();
        for f in self.events.values() {
            out.extend(f.search_events(query).await?);
        }
        Ok(out)
    }

    async fn fetch_event(&self, ext_id: &str) -> SourceResult<Sourced<SrcEvent>> {
        self.record(CallKind::FetchEvent, vec![ext_id.to_owned()])
            .await;
        match self.events.get(ext_id) {
            Some(f) => f.fetch_event(ext_id).await,
            None => Err(SourceError::NotFound(ext_id.to_owned())),
        }
    }

    async fn fetch_event_matches(&self, ext_id: &str) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        self.record(CallKind::FetchEventMatches, vec![ext_id.to_owned()])
            .await;
        match self.events.get(ext_id) {
            Some(f) => f.fetch_event_matches(ext_id).await,
            None => Ok(Vec::new()),
        }
    }

    async fn fetch_matches(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        self.record(CallKind::FetchMatches, ext_ids.to_vec()).await;
        Ok(self.matches_by_id(ext_ids, false))
    }

    async fn fetch_live(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        self.record(CallKind::FetchLive, ext_ids.to_vec()).await;
        Ok(self.matches_by_id(ext_ids, true))
    }
}

/// A database, a reconciler with `sim` as primary source, and the clock.
pub struct Env {
    pub pool: SqlitePool,
    pub reconciler: Arc<Reconciler>,
    pub clock: Arc<FakeClock>,
    pub source: Arc<SimSource>,
    pub _dir: Arc<tempfile::TempDir>,
}

pub async fn env(scenarios: Vec<Scenario>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("test.db").display());
    let pool = store::open(&url).await.unwrap();
    let clock = Arc::new(FakeClock::new(day()));
    Env {
        reconciler: reconciler(&pool),
        pool,
        source: Arc::new(SimSource::new(scenarios, clock.clone())),
        clock,
        _dir: Arc::new(dir),
    }
}

pub fn reconciler(pool: &SqlitePool) -> Arc<Reconciler> {
    Arc::new(Reconciler::new(
        pool.clone(),
        Precedence::from_enabled(&[SIM.to_owned()]),
        Some(SIM.to_owned()),
        EventBus::new(),
    ))
}

impl Env {
    /// A scheduler over the sim source, with a fixed jitter seed.
    pub async fn scheduler(&self, budget: Option<BudgetLimits>) -> (Scheduler, SyncHandle) {
        self.scheduler_with(budget, SyncConfig::default()).await
    }

    pub async fn scheduler_with(
        &self,
        budget: Option<BudgetLimits>,
        config: SyncConfig,
    ) -> (Scheduler, SyncHandle) {
        Scheduler::open(SchedulerParts {
            reconciler: self.reconciler.clone(),
            clock: self.clock.clone(),
            config,
            sources: vec![SourceEntry {
                adapter: self.source.clone(),
                budget,
            }],
            seed: Some(7),
        })
        .await
        .unwrap()
    }

    /// What T09 does when an admin follows an event: create it from the
    /// source (not counted as a scheduler call), follow it, and ask for an
    /// immediate structure sync.
    pub async fn follow(&self, handle: &SyncHandle, event: &str) -> EventId {
        let id = self.follow_quietly(event).await;
        handle.sync_event_now(id);
        id
    }

    /// Creates and follows an event without asking for a sync.
    pub async fn follow_quietly(&self, event: &str) -> EventId {
        let ev = self.source.fixture(event).fetch_event(event).await.unwrap();
        self.reconciler
            .ingest_event(SIM, &ev, self.clock.now())
            .await
            .unwrap();
        let id = EventId(
            store::refs::resolve(&self.pool, SIM, EntityType::Event, event)
                .await
                .unwrap()
                .unwrap(),
        );
        store::guilds::follow_event(
            &self.pool,
            &FollowedEvent {
                guild_id: GUILD.into(),
                event_id: id,
                followed_by: ADMIN.into(),
                followed_at: self.clock.now(),
            },
        )
        .await
        .unwrap();
        id
    }

    /// Loads every event's current state straight from the fixtures (not
    /// counted as calls), as if the scheduler had kept up until now.
    pub async fn catch_up(&self, events: &[&str]) {
        let now = self.clock.now();
        for e in events {
            let f = self.source.fixture(e);
            let ev = f.fetch_event(e).await.unwrap();
            self.reconciler.ingest_event(SIM, &ev, now).await.unwrap();
            let ms = f.fetch_event_matches(e).await.unwrap();
            let rep = self.reconciler.ingest_matches(SIM, &ms, now).await.unwrap();
            assert_eq!(rep.failed, 0);
        }
    }

    pub fn set_minute(&self, minute: i64) -> DateTime<Utc> {
        let t = day() + Duration::minutes(minute);
        self.clock.set(t);
        t
    }

    pub async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

/// The highest request cost within any window of `span` starting at a call.
pub fn max_in_window(calls: &[Call], span: Duration) -> u32 {
    let mut best = 0;
    for (i, c) in calls.iter().enumerate() {
        let sum: u32 = calls[i..]
            .iter()
            .take_while(|d| d.at - c.at < span)
            .map(|d| d.cost)
            .sum();
        best = best.max(sum);
    }
    best
}

/// When a match was asked for by a match-level call (not an event's full
/// match list).
pub fn polls_of(calls: &[Call], ext_id: &str) -> Vec<DateTime<Utc>> {
    calls
        .iter()
        .filter(|c| matches!(c.kind, CallKind::FetchMatches | CallKind::FetchLive))
        .filter(|c| c.ids.iter().any(|id| id == ext_id))
        .map(|c| c.at)
        .collect()
}
