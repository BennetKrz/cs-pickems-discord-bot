//! Data-source adapters.
//!
//! Adapters only fetch and convert external data into source-neutral DTOs
//! ([`dto`]); they never touch the database. The reconciler in the `sync`
//! crate turns DTOs into canonical data. Each adapter lives in its own module
//! behind a cargo feature: PandaScore (T04), manual and fixture (T05), and
//! the in-memory [`fake`] adapter for tests.
//!
//! See `docs/adr/0004-source-adapters-reconciliation.md`.

pub mod dto;
#[cfg(any(test, feature = "fake"))]
pub mod fake;

use std::time::Duration;

use async_trait::async_trait;

pub use dto::*;

/// What a source can deliver. The scheduler (T06) only asks a source for
/// data it declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// Match times and status.
    Schedule,
    /// Final series scores and winners.
    Results,
    /// Round scores per map.
    MapScores,
    /// Which map is played as map N.
    MapNames,
    /// Stages and bracket links between matches.
    Brackets,
    /// Updates while a match is running.
    Live,
    /// Event search by name (T09).
    Search,
}

/// The kinds of calls an adapter offers, for cost accounting (T06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallKind {
    SearchEvents,
    FetchEvent,
    FetchEventMatches,
    FetchMatches,
    FetchLive,
}

/// Request limits of the source's plan. The budget the scheduler actually
/// spends is configured separately (usually 50% of this).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimits {
    pub per_minute: u32,
    pub per_hour: u32,
}

/// Errors from an adapter call.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The source rejected the call for rate limiting (e.g. HTTP 429).
    #[error("rate limited by the source")]
    RateLimited { retry_after: Option<Duration> },
    /// The adapter does not offer this capability.
    #[error("source does not support {0:?}")]
    Unsupported(Capability),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("HTTP error: {0}")]
    Http(String),
    /// The source answered, but the payload could not be converted.
    #[error("cannot decode source payload: {0}")]
    Decode(String),
    #[error("{0}")]
    Other(String),
}

pub type SourceResult<T> = Result<T, SourceError>;

/// A source-neutral DTO plus the raw payload it was converted from. The
/// reconciler stores both in `source_records`; merging and replay only use
/// `dto`.
#[derive(Debug, Clone, PartialEq)]
pub struct Sourced<T> {
    pub dto: T,
    /// The source's own JSON for this entity, as received. `Null` if the
    /// source has no raw form (e.g. manual entries).
    pub raw: serde_json::Value,
}

impl<T> Sourced<T> {
    pub fn new(dto: T, raw: serde_json::Value) -> Self {
        Self { dto, raw }
    }
}

/// A data source. Implementations fetch and convert only; they never write
/// the database and never decide precedence.
#[async_trait]
pub trait SourceAdapter: Send + Sync {
    /// Stable source name, used in `external_refs`, `source_records`, config
    /// (`ENABLED_SOURCES`, precedence) and logs. Never `manual`.
    fn id(&self) -> &'static str;

    /// What this source can deliver.
    fn capabilities(&self) -> &[Capability];

    /// Limits of the source's plan.
    fn limits(&self) -> RateLimits;

    /// Requests one call costs; `items` is the number of IDs passed to a
    /// batch call (0 for others).
    fn cost(&self, call: CallKind, items: usize) -> u32;

    /// Events matching `query`, with tier and dates if known. Stages may be
    /// empty in search results.
    async fn search_events(&self, query: &str) -> SourceResult<Vec<Sourced<SrcEvent>>>;

    /// One event with its stages.
    async fn fetch_event(&self, ext_id: &str) -> SourceResult<Sourced<SrcEvent>>;

    /// All matches of an event, with teams, maps and bracket links.
    async fn fetch_event_matches(&self, ext_id: &str) -> SourceResult<Vec<Sourced<SrcMatch>>>;

    /// The given matches. Unknown IDs are skipped.
    async fn fetch_matches(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>>;

    /// Live state of the given matches. Unknown or not-live IDs are skipped.
    async fn fetch_live(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>>;

    /// Whether this source declares `cap`.
    fn supports(&self, cap: Capability) -> bool {
        self.capabilities().contains(&cap)
    }
}
