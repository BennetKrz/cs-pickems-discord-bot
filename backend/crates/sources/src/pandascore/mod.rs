//! The PandaScore adapter (feature `pandascore`).
//!
//! Uses PandaScore's REST API on the free plan ("Schedules, Results &
//! Context Data"). CS2 routes still use the `csgo` slug. A PandaScore
//! *serie* is our event and its *tournaments* are our stages.
//!
//! The free plan has schedules, statuses, series scores, per-map winners
//! and bracket links, but **no map names and no round scores** (the game
//! detail routes answer 403). So this adapter does not declare
//! [`Capability::MapScores`] or [`Capability::MapNames`]; those come from
//! manual entry (T05). See `docs/adr/0001-pandascore-coverage.md`.
//!
//! Requests are capped at the plan's limits inside the adapter, whatever
//! the scheduler's budget says, and HTTP 429 is retried with exponential
//! backoff and jitter.

mod client;
mod convert;
mod limiter;
mod model;

#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use client::Client;
pub use client::{MAX_PAGES, PAGE_SIZE, RetryPolicy};
use convert::{FeedLookup, convert_match, decode_match, decode_serie, is_cs, serie_to_event};

use crate::{
    CallKind, Capability, Quota, RateLimits, SourceAdapter, SourceError, SourceResult, Sourced,
    SrcEvent, SrcMatch,
};

/// The source name of this adapter.
pub const PANDASCORE: &str = "pandascore";

/// The production API.
pub const DEFAULT_BASE_URL: &str = "https://api.pandascore.co";

/// Limits of the free plan: 1,000 requests per hour and a burst cap of 60
/// per minute (checked 2026-11-17).
pub const FREE_PLAN: RateLimits = RateLimits {
    per_minute: 60,
    per_hour: 1000,
};

/// Search results per query.
const SEARCH_LIMIT: usize = 25;

/// Bracket stages assumed per event when estimating the cost of
/// `fetch_event_matches`.
const ASSUMED_BRACKET_STAGES: u32 = 2;

/// Settings of the adapter. The token comes from `PANDASCORE_TOKEN`.
#[derive(Clone)]
pub struct PandascoreConfig {
    pub token: String,
    pub base_url: String,
    /// The plan's limits; the adapter never sends more.
    pub limits: RateLimits,
    /// Per-request timeout; `None` for none.
    pub timeout: Option<Duration>,
    pub retry: RetryPolicy,
}

impl PandascoreConfig {
    /// Production defaults: the public API, the free plan's limits, a 20 s
    /// timeout and three retries after 429.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            limits: FREE_PLAN,
            timeout: Some(Duration::from_secs(20)),
            retry: RetryPolicy::default(),
        }
    }
}

impl fmt::Debug for PandascoreConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PandascoreConfig")
            .field("token", &"<redacted>")
            .field("base_url", &self.base_url)
            .field("limits", &self.limits)
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .finish()
    }
}

pub struct PandascoreAdapter {
    client: Client,
    limits: RateLimits,
}

impl fmt::Debug for PandascoreAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PandascoreAdapter")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl PandascoreAdapter {
    pub fn new(config: PandascoreConfig) -> SourceResult<Self> {
        Ok(Self {
            client: Client::new(&config)?,
            limits: config.limits,
        })
    }

    /// Fetches matches by ID from `path`, in batches of [`PAGE_SIZE`].
    async fn matches_by_id(
        &self,
        path: &str,
        ext_ids: &[String],
    ) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        let ids: BTreeSet<u64> = ext_ids.iter().filter_map(|id| id.parse().ok()).collect();
        let ids: Vec<u64> = ids.into_iter().collect();
        let mut raws = Vec::new();
        for chunk in ids.chunks(PAGE_SIZE) {
            let list = chunk
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            raws.extend(self.client.get_all(path, &[("filter[id]", list)]).await?);
        }
        Ok(convert_all(raws, &FeedLookup::default()))
    }
}

/// Decodes and converts matches; a match that cannot be decoded is logged
/// and skipped, so one bad entry does not block the rest.
fn convert_all(raws: Vec<Value>, lookup: &FeedLookup) -> Vec<Sourced<SrcMatch>> {
    raws.into_iter()
        .filter_map(|raw| {
            let m = decode_match(&raw)
                .map_err(|e| tracing::warn!(error = %e, "skipping PandaScore match"))
                .ok()?;
            let dto = convert_match(&m, lookup)?;
            Some(Sourced::new(dto, raw))
        })
        .collect()
}

fn parse_id(ext_id: &str, what: &str) -> SourceResult<u64> {
    ext_id
        .parse()
        .map_err(|_| SourceError::NotFound(format!("{what} {ext_id}")))
}

#[async_trait]
impl SourceAdapter for PandascoreAdapter {
    fn id(&self) -> &'static str {
        PANDASCORE
    }

    fn capabilities(&self) -> &[Capability] {
        // No MapScores / MapNames on the free plan (ADR 0001). Live means
        // REST polling of running matches, not the websocket feed.
        &[
            Capability::Schedule,
            Capability::Results,
            Capability::Brackets,
            Capability::Live,
            Capability::Search,
        ]
    }

    fn limits(&self) -> RateLimits {
        self.limits
    }

    /// Estimated requests. Extra pages of very long listings (more than
    /// [`PAGE_SIZE`] matches in one event) cost one more each.
    fn cost(&self, call: CallKind, items: usize) -> u32 {
        match call {
            CallKind::SearchEvents | CallKind::FetchEvent => 1,
            CallKind::FetchEventMatches => 1 + ASSUMED_BRACKET_STAGES,
            CallKind::FetchMatches | CallKind::FetchLive => {
                u32::try_from(items.div_ceil(PAGE_SIZE)).unwrap_or(u32::MAX)
            }
        }
    }

    fn quota(&self) -> Option<Quota> {
        self.client.quota()
    }

    async fn search_events(&self, query: &str) -> SourceResult<Vec<Sourced<SrcEvent>>> {
        // Serie slugs contain the league name ("cs-go-blast-rivals-fall-2026"),
        // so a slug search finds events by league and season.
        let slug = convert::slugify(query);
        if slug.is_empty() {
            return Ok(Vec::new());
        }
        let body = self
            .client
            .get(
                "/csgo/series",
                &[
                    ("search[slug]", slug),
                    ("sort", "-begin_at".to_owned()),
                    ("per_page", SEARCH_LIMIT.to_string()),
                ],
            )
            .await?;
        let Value::Array(items) = body else {
            return Err(SourceError::Decode("/csgo/series: expected a list".into()));
        };
        Ok(items
            .into_iter()
            .filter_map(|raw| {
                let s = decode_serie(&raw).ok().filter(is_cs)?;
                Some(Sourced::new(serie_to_event(&s), raw))
            })
            .collect())
    }

    async fn fetch_event(&self, ext_id: &str) -> SourceResult<Sourced<SrcEvent>> {
        let id = parse_id(ext_id, "serie")?;
        let raw = self.client.get(&format!("/series/{id}"), &[]).await?;
        let serie = decode_serie(&raw)?;
        if !is_cs(&serie) {
            return Err(SourceError::NotFound(format!(
                "serie {id} is not Counter-Strike"
            )));
        }
        Ok(Sourced::new(serie_to_event(&serie), raw))
    }

    async fn fetch_event_matches(&self, ext_id: &str) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        let id = parse_id(ext_id, "serie")?;
        let mut raws = self
            .client
            .get_all(
                "/csgo/matches",
                &[("filter[serie_id]", id.to_string()), ("sort", "id".into())],
            )
            .await?;

        // Bracket links are only on the brackets route, one call per
        // bracket stage. Which stages have one is in the embedded tournament.
        let decoded: Vec<Option<model::PsMatch>> =
            raws.iter().map(|r| decode_match(r).ok()).collect();
        let bracket_stages: BTreeSet<u64> = decoded
            .iter()
            .flatten()
            .filter_map(|m| m.tournament.as_ref())
            .filter(|t| t.has_bracket == Some(true))
            .map(|t| t.id)
            .collect();
        let mut links: HashMap<u64, Value> = HashMap::new();
        for stage in &bracket_stages {
            let items = self
                .client
                .get_all(&format!("/tournaments/{stage}/brackets"), &[])
                .await?;
            for item in items {
                if let (Some(mid), Some(prev)) = (
                    item.get("id").and_then(Value::as_u64),
                    item.get("previous_matches"),
                ) {
                    links.insert(mid, prev.clone());
                }
            }
        }
        for (raw, m) in raws.iter_mut().zip(&decoded) {
            let Some(m) = m else { continue };
            let in_bracket = m
                .tournament
                .as_ref()
                .is_some_and(|t| bracket_stages.contains(&t.id));
            if let (true, Value::Object(obj)) = (in_bracket, raw) {
                let prev = links
                    .get(&m.id)
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new()));
                obj.insert("previous_matches".into(), prev);
            }
        }

        let all: Vec<model::PsMatch> = raws.iter().filter_map(|r| decode_match(r).ok()).collect();
        let lookup = FeedLookup::new(&all);
        Ok(convert_all(raws, &lookup))
    }

    async fn fetch_matches(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        self.matches_by_id("/csgo/matches", ext_ids).await
    }

    async fn fetch_live(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        self.matches_by_id("/csgo/matches/running", ext_ids).await
    }
}
