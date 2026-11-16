//! An in-memory adapter for tests (feature `fake`).
//!
//! Tests set events and matches directly; every call returns the current
//! state and is counted, so budget logic (T06) can be checked.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;

use crate::{
    CallKind, Capability, RateLimits, SourceAdapter, SourceError, SourceResult, Sourced, SrcEvent,
    SrcMatch,
};

/// A source backed by maps of DTOs. The raw payload of each entity is its
/// DTO serialized as JSON, wrapped as `{"fake": …}`.
pub struct FakeAdapter {
    id: &'static str,
    capabilities: Vec<Capability>,
    limits: RateLimits,
    events: Mutex<BTreeMap<String, SrcEvent>>,
    matches: Mutex<BTreeMap<String, SrcMatch>>,
    calls: AtomicU32,
}

impl FakeAdapter {
    /// An empty source with every capability and a 60/min, 1000/h plan.
    pub fn new(id: &'static str) -> Self {
        Self {
            id,
            capabilities: vec![
                Capability::Schedule,
                Capability::Results,
                Capability::MapScores,
                Capability::MapNames,
                Capability::Brackets,
                Capability::Live,
                Capability::Search,
            ],
            limits: RateLimits {
                per_minute: 60,
                per_hour: 1000,
            },
            events: Mutex::default(),
            matches: Mutex::default(),
            calls: AtomicU32::new(0),
        }
    }

    pub fn with_capabilities(mut self, caps: &[Capability]) -> Self {
        self.capabilities = caps.to_vec();
        self
    }

    /// Adds or replaces an event.
    pub fn put_event(&self, e: SrcEvent) {
        self.events.lock().unwrap().insert(e.ext_id.clone(), e);
    }

    /// Adds or replaces a match.
    pub fn put_match(&self, m: SrcMatch) {
        self.matches.lock().unwrap().insert(m.ext_id.clone(), m);
    }

    /// Number of fetch calls so far.
    pub fn calls(&self) -> u32 {
        self.calls.load(Ordering::Relaxed)
    }

    fn count(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    fn require(&self, cap: Capability) -> SourceResult<()> {
        if self.capabilities.contains(&cap) {
            Ok(())
        } else {
            Err(SourceError::Unsupported(cap))
        }
    }
}

/// Wraps a DTO with its fake raw payload.
pub fn sourced<T: serde::Serialize + Clone>(dto: &T) -> Sourced<T> {
    let raw = serde_json::json!({ "fake": dto });
    Sourced::new(dto.clone(), raw)
}

#[async_trait]
impl SourceAdapter for FakeAdapter {
    fn id(&self) -> &'static str {
        self.id
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    fn limits(&self) -> RateLimits {
        self.limits
    }

    fn cost(&self, _call: CallKind, _items: usize) -> u32 {
        1
    }

    async fn search_events(&self, query: &str) -> SourceResult<Vec<Sourced<SrcEvent>>> {
        self.require(Capability::Search)?;
        self.count();
        let q = query.to_lowercase();
        Ok(self
            .events
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.name.to_lowercase().contains(&q))
            .map(sourced)
            .collect())
    }

    async fn fetch_event(&self, ext_id: &str) -> SourceResult<Sourced<SrcEvent>> {
        self.count();
        self.events
            .lock()
            .unwrap()
            .get(ext_id)
            .map(sourced)
            .ok_or_else(|| SourceError::NotFound(format!("event {ext_id}")))
    }

    async fn fetch_event_matches(&self, ext_id: &str) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        self.count();
        Ok(self
            .matches
            .lock()
            .unwrap()
            .values()
            .filter(|m| m.event_ext_id == ext_id)
            .map(sourced)
            .collect())
    }

    async fn fetch_matches(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        self.count();
        let matches = self.matches.lock().unwrap();
        Ok(ext_ids
            .iter()
            .filter_map(|id| matches.get(id))
            .map(sourced)
            .collect())
    }

    async fn fetch_live(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        self.require(Capability::Live)?;
        self.count();
        let matches = self.matches.lock().unwrap();
        Ok(ext_ids
            .iter()
            .filter_map(|id| matches.get(id))
            .filter(|m| m.status == Some(domain::MatchStatus::Live))
            .map(sourced)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SrcTeam;

    #[tokio::test]
    async fn returns_what_was_put_and_counts_calls() {
        let src = FakeAdapter::new("alpha");
        src.put_event(SrcEvent::new("e1", "IEM Test"));
        src.put_match(SrcMatch {
            team_a: Some(SrcTeam::named("t1", "Alpha")),
            ..SrcMatch::new("m1", "e1")
        });
        src.put_match(SrcMatch::new("m2", "e2"));

        let dyn_src: &dyn SourceAdapter = &src;
        assert_eq!(dyn_src.id(), "alpha");
        assert_eq!(
            dyn_src.fetch_event("e1").await.unwrap().dto.name,
            "IEM Test"
        );
        let ms = dyn_src.fetch_event_matches("e1").await.unwrap();
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].raw["fake"]["ext_id"], "m1");
        assert!(matches!(
            dyn_src.fetch_event("nope").await,
            Err(SourceError::NotFound(_))
        ));
        assert_eq!(dyn_src.search_events("iem").await.unwrap().len(), 1);
        assert_eq!(src.calls(), 4);
    }

    #[tokio::test]
    async fn undeclared_capabilities_are_refused() {
        let src = FakeAdapter::new("beta").with_capabilities(&[Capability::Schedule]);
        assert!(!src.supports(Capability::Live));
        assert!(matches!(
            src.fetch_live(&["m1".into()]).await,
            Err(SourceError::Unsupported(Capability::Live))
        ));
        assert_eq!(src.calls(), 0);
    }
}
