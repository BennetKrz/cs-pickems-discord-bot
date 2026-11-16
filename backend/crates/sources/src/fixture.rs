//! The `fixture` source for development and tests (feature `fixture`).
//!
//! A scenario file describes one event with fixed results; all times are
//! minutes after an **anchor**. The adapter computes every match's state at
//! the clock's current time, so moving a [`domain::FakeClock`] drives the
//! whole lifecycle through the normal reconciler: scheduled, live with maps
//! finishing one by one, finished (or canceled). Bracket slots fed by an
//! earlier match stay TBD until that match finishes.
//!
//! The default scenario (`fixtures/scenario.json`) has a Swiss stage and a
//! double-elimination playoff, Bo1/Bo3/Bo5 matches, TBD slots, a canceled
//! match and overtime maps. No API quota is used: the cost of every call is 0.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use domain::{
    BracketSide, Clock, EventStatus, EventTier, FeedOutcome, MapStatus, MatchFormat, MatchStatus,
    Side, StageKind,
};
use serde::{Deserialize, Serialize};

use crate::{
    CallKind, Capability, RateLimits, SourceAdapter, SourceError, SourceResult, Sourced,
    SrcBracket, SrcEvent, SrcFeed, SrcMap, SrcMatch, SrcStage, SrcTeam,
};

/// The source name of this adapter.
pub const FIXTURE: &str = "fixture";

/// The built-in scenario.
pub const DEFAULT_SCENARIO: &str = include_str!("../fixtures/scenario.json");

/// Minutes a map takes unless the scenario says otherwise.
pub const DEFAULT_MAP_MINUTES: i64 = 45;

/// A scenario: one event, its teams and its matches with fixed results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub event: ScenarioEvent,
    pub teams: Vec<ScenarioTeam>,
    /// A match may only be fed by matches listed before it.
    pub matches: Vec<ScenarioMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioEvent {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<EventTier>,
    pub stages: Vec<ScenarioStage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioStage {
    pub id: String,
    pub name: String,
    pub kind: StageKind,
    #[serde(default)]
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioTeam {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acronym: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
}

/// A slot of a match: a team ID, or the winner/loser of an earlier match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Slot {
    Team(String),
    Feed {
        #[serde(rename = "match")]
        match_id: String,
        outcome: FeedOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioBracket {
    pub side: BracketSide,
    pub round: i64,
    pub slot: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioMatch {
    pub id: String,
    pub stage: String,
    pub format: MatchFormat,
    /// Minutes after the anchor.
    pub start: i64,
    pub team_a: Slot,
    pub team_b: Slot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bracket: Option<ScenarioBracket>,
    /// Minutes after the anchor when the match is canceled. Must not be
    /// after `start`; a canceled match has no maps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_at: Option<i64>,
    /// The maps that are played, in order. Later maps are not played.
    pub maps: Vec<ScenarioMap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioMap {
    pub name: String,
    /// Final rounds `(a, b)`; the side with more rounds wins the map.
    pub rounds: (u16, u16),
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<i64>,
}

impl ScenarioMap {
    fn minutes(&self) -> i64 {
        self.minutes.unwrap_or(DEFAULT_MAP_MINUTES)
    }

    fn winner(&self) -> Side {
        if self.rounds.0 > self.rounds.1 {
            Side::A
        } else {
            Side::B
        }
    }
}

impl ScenarioMatch {
    /// Minutes after the anchor when the last map ends.
    pub fn end(&self) -> i64 {
        self.start + self.maps.iter().map(ScenarioMap::minutes).sum::<i64>()
    }

    /// The side that wins the series, if the match is played.
    pub fn winner(&self) -> Option<Side> {
        let a = self.maps.iter().filter(|m| m.winner() == Side::A).count();
        let b = self.maps.len() - a;
        match a.cmp(&b) {
            std::cmp::Ordering::Greater => Some(Side::A),
            std::cmp::Ordering::Less => Some(Side::B),
            std::cmp::Ordering::Equal => None,
        }
    }
}

impl Scenario {
    /// The built-in scenario.
    pub fn default_scenario() -> Self {
        Self::from_json(DEFAULT_SCENARIO).expect("the built-in scenario is valid")
    }

    /// Parses and validates a scenario.
    pub fn from_json(json: &str) -> Result<Self, SourceError> {
        let s: Scenario =
            serde_json::from_str(json).map_err(|e| SourceError::Decode(e.to_string()))?;
        s.validate().map_err(SourceError::Decode)?;
        Ok(s)
    }

    fn validate(&self) -> Result<(), String> {
        let stages: Vec<&str> = self.event.stages.iter().map(|s| s.id.as_str()).collect();
        let teams: Vec<&str> = self.teams.iter().map(|t| t.id.as_str()).collect();
        let mut seen: Vec<&str> = Vec::new();
        for m in &self.matches {
            let at = |msg: String| format!("match {}: {msg}", m.id);
            if seen.contains(&m.id.as_str()) {
                return Err(at("duplicate ID".into()));
            }
            if !stages.contains(&m.stage.as_str()) {
                return Err(at(format!("unknown stage {}", m.stage)));
            }
            for slot in [&m.team_a, &m.team_b] {
                match slot {
                    Slot::Team(t) if !teams.contains(&t.as_str()) => {
                        return Err(at(format!("unknown team {t}")));
                    }
                    Slot::Feed { match_id, .. } if !seen.contains(&match_id.as_str()) => {
                        return Err(at(format!(
                            "fed by {match_id}, which is not listed before it"
                        )));
                    }
                    _ => {}
                }
            }
            if m.team_a == m.team_b {
                return Err(at("both slots are the same".into()));
            }
            if let Some(c) = m.cancel_at {
                if c > m.start || !m.maps.is_empty() {
                    return Err(at(
                        "a canceled match has no maps and is canceled by its start".into(),
                    ));
                }
            } else {
                let wins = m.format.wins_needed();
                let (mut a, mut b) = (0, 0);
                for (i, map) in m.maps.iter().enumerate() {
                    if map.rounds.0 == map.rounds.1 {
                        return Err(at(format!("map {} has no winner", i + 1)));
                    }
                    if a == wins || b == wins {
                        return Err(at(format!(
                            "map {} is played after the series is decided",
                            i + 1
                        )));
                    }
                    match map.winner() {
                        Side::A => a += 1,
                        Side::B => b += 1,
                    }
                }
                if a != wins && b != wins {
                    return Err(at(format!("the maps do not decide the {}", m.format)));
                }
            }
            seen.push(&m.id);
        }
        Ok(())
    }
}

/// The adapter. Cheap to share; the state lives in the clock.
pub struct FixtureAdapter {
    scenario: Scenario,
    anchor: DateTime<Utc>,
    clock: Arc<dyn Clock>,
}

impl FixtureAdapter {
    /// `anchor` is the time all scenario offsets count from.
    pub fn new(scenario: Scenario, anchor: DateTime<Utc>, clock: Arc<dyn Clock>) -> Self {
        Self {
            scenario,
            anchor,
            clock,
        }
    }

    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }

    pub fn anchor(&self) -> DateTime<Utc> {
        self.anchor
    }

    /// The scenario event's external ID.
    pub fn event_id(&self) -> &str {
        &self.scenario.event.id
    }

    fn at(&self, minutes: i64) -> DateTime<Utc> {
        self.anchor + Duration::minutes(minutes)
    }

    /// The event as of now.
    pub fn event(&self) -> SrcEvent {
        let now = self.clock.now();
        let begin = self.scenario.matches.iter().map(|m| m.start).min();
        let end = self.scenario.matches.iter().map(ScenarioMatch::end).max();
        let status = match (begin, end) {
            (Some(b), _) if now < self.at(b) => EventStatus::Upcoming,
            (_, Some(e)) if now >= self.at(e) => EventStatus::Finished,
            (Some(_), _) => EventStatus::Running,
            _ => EventStatus::Upcoming,
        };
        let e = &self.scenario.event;
        SrcEvent {
            ext_id: e.id.clone(),
            name: e.name.clone(),
            tier: e.tier,
            begin_at: begin.map(|b| self.at(b)),
            end_at: end.map(|e| self.at(e)),
            status: Some(status),
            stages: e
                .stages
                .iter()
                .map(|s| SrcStage {
                    ext_id: s.id.clone(),
                    name: s.name.clone(),
                    kind: Some(s.kind),
                    position: s.position,
                })
                .collect(),
        }
    }

    /// Every match as of now, in scenario order.
    pub fn matches(&self) -> Vec<SrcMatch> {
        let now = self.clock.now();
        // Team IDs of finished matches' winners and losers, for feeds.
        let mut results: BTreeMap<&str, (Option<String>, Option<String>)> = BTreeMap::new();
        let mut out = Vec::new();
        for m in &self.scenario.matches {
            let team = |slot: &Slot| match slot {
                Slot::Team(t) => Some(t.clone()),
                Slot::Feed { match_id, outcome } => {
                    let (w, l) = results.get(match_id.as_str()).cloned().unwrap_or_default();
                    match outcome {
                        FeedOutcome::Winner => w,
                        FeedOutcome::Loser => l,
                    }
                }
            };
            let (team_a, team_b) = (team(&m.team_a), team(&m.team_b));
            let dto = self.match_at(m, team_a.as_deref(), team_b.as_deref(), now);
            if dto.status == Some(MatchStatus::Finished) {
                let (w, l) = match dto.winner {
                    Some(Side::A) => (team_a, team_b),
                    _ => (team_b, team_a),
                };
                results.insert(&m.id, (w, l));
            }
            out.push(dto);
        }
        out
    }

    fn team(&self, id: Option<&str>) -> Option<SrcTeam> {
        let t = self
            .scenario
            .teams
            .iter()
            .find(|t| Some(t.id.as_str()) == id)?;
        Some(SrcTeam {
            ext_id: t.id.clone(),
            name: t.name.clone(),
            acronym: t.acronym.clone(),
            logo_url: None,
            country: t.country.clone(),
        })
    }

    fn match_at(
        &self,
        m: &ScenarioMatch,
        team_a: Option<&str>,
        team_b: Option<&str>,
        now: DateTime<Utc>,
    ) -> SrcMatch {
        let feed = |slot: &Slot| match slot {
            Slot::Feed { match_id, outcome } => Some(SrcFeed {
                match_ext_id: match_id.clone(),
                outcome: *outcome,
            }),
            Slot::Team(_) => None,
        };
        let bracket = m.bracket.as_ref().map(|b| SrcBracket {
            side: Some(b.side),
            round_index: Some(b.round),
            slot_index: Some(b.slot),
            feeds_a: feed(&m.team_a),
            feeds_b: feed(&m.team_b),
        });
        let start = self.at(m.start);
        let mut dto = SrcMatch {
            stage_ext_id: Some(m.stage.clone()),
            team_a: self.team(team_a),
            team_b: self.team(team_b),
            format: Some(m.format),
            status: Some(MatchStatus::Scheduled),
            scheduled_at: Some(start),
            bracket,
            ..SrcMatch::new(m.id.clone(), self.scenario.event.id.clone())
        };

        if let Some(c) = m.cancel_at {
            if now >= self.at(c) {
                dto.status = Some(MatchStatus::Canceled);
            }
            return dto;
        }
        if now < start {
            return dto;
        }

        // Started: walk the maps in order.
        dto.started_at = Some(start);
        let (mut won_a, mut won_b) = (0u8, 0u8);
        let mut map_end = start;
        let mut all_done = true;
        for (i, map) in m.maps.iter().enumerate() {
            let number = u8::try_from(i + 1).expect("at most 5 maps");
            let map_start = map_end;
            map_end = map_start + Duration::minutes(map.minutes());
            let mut src = SrcMap {
                name: Some(map.name.clone()),
                ..SrcMap::new(number)
            };
            if now >= map_end {
                src.status = Some(MapStatus::Finished);
                src.rounds = Some(map.rounds);
                src.winner = Some(map.winner());
                match map.winner() {
                    Side::A => won_a += 1,
                    Side::B => won_b += 1,
                }
            } else {
                all_done = false;
                src.status = Some(if now >= map_start {
                    MapStatus::Live
                } else {
                    MapStatus::Upcoming
                });
            }
            dto.maps.push(src);
        }
        dto.score = Some((won_a, won_b));
        if all_done {
            dto.status = Some(MatchStatus::Finished);
            dto.ended_at = Some(map_end);
            dto.winner = m.winner();
            for n in m.maps.len() + 1..=usize::from(m.format.max_maps()) {
                let number = u8::try_from(n).expect("at most 5 maps");
                dto.maps.push(SrcMap {
                    status: Some(MapStatus::NotPlayed),
                    ..SrcMap::new(number)
                });
            }
        } else {
            dto.status = Some(MatchStatus::Live);
        }
        dto
    }

    fn raw_match(&self, id: &str) -> serde_json::Value {
        let spec = self.scenario.matches.iter().find(|m| m.id == id);
        serde_json::json!({ "fixture": spec, "anchor": self.anchor, "at": self.clock.now() })
    }

    fn sourced_match(&self, dto: SrcMatch) -> Sourced<SrcMatch> {
        let raw = self.raw_match(&dto.ext_id);
        Sourced::new(dto, raw)
    }
}

#[async_trait]
impl SourceAdapter for FixtureAdapter {
    fn id(&self) -> &'static str {
        FIXTURE
    }

    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::Schedule,
            Capability::Results,
            Capability::MapScores,
            Capability::MapNames,
            Capability::Brackets,
            Capability::Live,
            Capability::Search,
        ]
    }

    fn limits(&self) -> RateLimits {
        RateLimits {
            per_minute: u32::MAX,
            per_hour: u32::MAX,
        }
    }

    /// Fixtures are local: no call costs anything.
    fn cost(&self, _call: CallKind, _items: usize) -> u32 {
        0
    }

    async fn search_events(&self, query: &str) -> SourceResult<Vec<Sourced<SrcEvent>>> {
        let e = self.event();
        if e.name.to_lowercase().contains(&query.to_lowercase()) {
            let raw = serde_json::json!({ "fixture": &self.scenario.event });
            Ok(vec![Sourced::new(e, raw)])
        } else {
            Ok(Vec::new())
        }
    }

    async fn fetch_event(&self, ext_id: &str) -> SourceResult<Sourced<SrcEvent>> {
        if ext_id != self.event_id() {
            return Err(SourceError::NotFound(format!("event {ext_id}")));
        }
        let raw = serde_json::json!({ "fixture": &self.scenario.event, "anchor": self.anchor });
        Ok(Sourced::new(self.event(), raw))
    }

    async fn fetch_event_matches(&self, ext_id: &str) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        if ext_id != self.event_id() {
            return Ok(Vec::new());
        }
        Ok(self
            .matches()
            .into_iter()
            .map(|m| self.sourced_match(m))
            .collect())
    }

    async fn fetch_matches(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        Ok(self
            .matches()
            .into_iter()
            .filter(|m| ext_ids.contains(&m.ext_id))
            .map(|m| self.sourced_match(m))
            .collect())
    }

    async fn fetch_live(&self, ext_ids: &[String]) -> SourceResult<Vec<Sourced<SrcMatch>>> {
        Ok(self
            .matches()
            .into_iter()
            .filter(|m| ext_ids.contains(&m.ext_id) && m.status == Some(MatchStatus::Live))
            .map(|m| self.sourced_match(m))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use domain::FakeClock;

    fn anchor() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap()
    }

    fn adapter() -> (FixtureAdapter, Arc<FakeClock>) {
        let clock = Arc::new(FakeClock::new(anchor()));
        (
            FixtureAdapter::new(Scenario::default_scenario(), anchor(), clock.clone()),
            clock,
        )
    }

    fn get(a: &FixtureAdapter, id: &str) -> SrcMatch {
        a.matches().into_iter().find(|m| m.ext_id == id).unwrap()
    }

    fn at_min(clock: &FakeClock, minutes: i64) {
        clock.set(anchor() + Duration::minutes(minutes));
    }

    #[test]
    fn default_scenario_covers_the_required_cases() {
        let s = Scenario::default_scenario();
        let kinds: Vec<_> = s.event.stages.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [StageKind::Swiss, StageKind::DoubleElim]);
        for f in MatchFormat::ALL {
            assert!(s.matches.iter().any(|m| m.format == *f), "{f}");
        }
        assert!(s.matches.iter().any(|m| m.cancel_at.is_some()));
        assert!(
            s.matches
                .iter()
                .any(|m| matches!(m.team_a, Slot::Feed { .. }))
        );
        let overtime = |m: &ScenarioMap| m.rounds.0.max(m.rounds.1) > 13;
        assert!(s.matches.iter().flat_map(|m| &m.maps).any(overtime));
    }

    #[test]
    fn a_match_goes_from_scheduled_to_live_to_finished() {
        let (a, clock) = adapter();
        let start = anchor() + Duration::minutes(180);

        // sw-r2-1: Bo3 at +180, maps of 45 min, 2:1 for A after 135 min.
        let m = get(&a, "sw-r2-1");
        assert_eq!(m.status, Some(MatchStatus::Scheduled));
        assert_eq!(m.scheduled_at, Some(start));
        assert!(m.maps.is_empty() && m.score.is_none());

        at_min(&clock, 180);
        let m = get(&a, "sw-r2-1");
        assert_eq!(m.status, Some(MatchStatus::Live));
        assert_eq!(m.started_at, Some(start));
        assert_eq!(m.score, Some((0, 0)));
        let st: Vec<_> = m.maps.iter().map(|m| m.status.unwrap()).collect();
        assert_eq!(
            st,
            [MapStatus::Live, MapStatus::Upcoming, MapStatus::Upcoming]
        );

        at_min(&clock, 180 + 90);
        let m = get(&a, "sw-r2-1");
        assert_eq!(m.score, Some((1, 1)));
        assert_eq!(m.maps[1].rounds, Some((14, 16)), "overtime map");
        assert_eq!(m.maps[1].winner, Some(Side::B));
        assert_eq!(m.maps[2].status, Some(MapStatus::Live));

        at_min(&clock, 180 + 135);
        let m = get(&a, "sw-r2-1");
        assert_eq!(m.status, Some(MatchStatus::Finished));
        assert_eq!((m.score, m.winner), (Some((2, 1)), Some(Side::A)));
        assert_eq!(m.ended_at, Some(start + Duration::minutes(135)));
    }

    #[test]
    fn decided_series_marks_the_rest_not_played() {
        let (a, clock) = adapter();
        at_min(&clock, 10_000);
        let m = get(&a, "sw-r2-2");
        let st: Vec<_> = m.maps.iter().map(|m| m.status.unwrap()).collect();
        assert_eq!(
            st,
            [
                MapStatus::Finished,
                MapStatus::Finished,
                MapStatus::NotPlayed
            ]
        );
        let gf = get(&a, "gf");
        assert_eq!(gf.format, Some(MatchFormat::Bo5));
        assert_eq!((gf.score, gf.winner), (Some((3, 1)), Some(Side::A)));
        assert_eq!(gf.maps[2].rounds, Some((19, 17)));
        assert_eq!(gf.maps[4].status, Some(MapStatus::NotPlayed));
        assert_eq!(a.event().status, Some(EventStatus::Finished));
    }

    #[test]
    fn cancellation_and_tbd_slots() {
        let (a, clock) = adapter();
        assert_eq!(get(&a, "sw-r1-4").status, Some(MatchStatus::Scheduled));
        at_min(&clock, 60);
        assert_eq!(get(&a, "sw-r1-4").status, Some(MatchStatus::Canceled));

        // The upper final is TBD until both semis are done.
        let uf = get(&a, "ub-f");
        assert!(uf.team_a.is_none() && uf.team_b.is_none());
        let b = uf.bracket.unwrap();
        assert_eq!(b.side, Some(BracketSide::Upper));
        assert_eq!(b.feeds_a.unwrap().match_ext_id, "ub-sf1");

        // ub-sf1 (2 maps) ends at +570, ub-sf2 (3 maps) at +615.
        at_min(&clock, 570);
        let uf = get(&a, "ub-f");
        assert_eq!(uf.team_a.unwrap().ext_id, "aur");
        assert!(uf.team_b.is_none());
        let lb = get(&a, "lb-r1");
        assert_eq!(lb.team_a.unwrap().ext_id, "fjo", "loser of ub-sf1");

        at_min(&clock, 615);
        assert_eq!(get(&a, "ub-f").team_b.unwrap().ext_id, "cin");
        assert_eq!(get(&a, "lb-r1").team_b.unwrap().ext_id, "gla");

        at_min(&clock, 10_000);
        let gf = get(&a, "gf");
        assert_eq!(
            (gf.team_a.unwrap().ext_id, gf.team_b.unwrap().ext_id),
            ("aur".to_owned(), "cin".to_owned())
        );
    }

    #[tokio::test]
    async fn adapter_calls() {
        let (a, clock) = adapter();
        let dyn_a: &dyn SourceAdapter = &a;
        assert_eq!(dyn_a.id(), "fixture");
        assert_eq!(dyn_a.cost(CallKind::FetchEventMatches, 0), 0);
        let e = dyn_a.fetch_event("fx-major").await.unwrap();
        assert_eq!(e.dto.stages.len(), 2);
        assert_eq!(e.dto.status, Some(EventStatus::Upcoming));
        assert!(dyn_a.fetch_event("nope").await.is_err());
        assert_eq!(dyn_a.search_events("masters").await.unwrap().len(), 1);
        assert!(dyn_a.search_events("nothing").await.unwrap().is_empty());
        let all = dyn_a.fetch_event_matches("fx-major").await.unwrap();
        assert_eq!(all.len(), 12);
        assert_eq!(all[0].raw["fixture"]["id"], "sw-r1-1");

        at_min(&clock, 40);
        let ids = vec!["sw-r1-1".to_owned(), "sw-r1-3".to_owned()];
        assert_eq!(dyn_a.fetch_matches(&ids).await.unwrap().len(), 2);
        let live = dyn_a.fetch_live(&ids).await.unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].dto.ext_id, "sw-r1-1");
        assert_eq!(a.event().status, Some(EventStatus::Running));
    }

    #[test]
    fn invalid_scenarios_are_rejected() {
        let base = || serde_json::from_str::<serde_json::Value>(DEFAULT_SCENARIO).unwrap();
        let check = |v: serde_json::Value, needle: &str| {
            let err = Scenario::from_json(&v.to_string()).unwrap_err().to_string();
            assert!(err.contains(needle), "{err}");
        };

        let mut v = base();
        v["matches"][0]["team_a"] = "nobody".into();
        check(v, "unknown team nobody");

        let mut v = base();
        v["matches"][0]["maps"] = serde_json::json!([]);
        check(v, "do not decide");

        let mut v = base();
        // sw-r2-2 is already 2:0 after two maps.
        v["matches"][5]["maps"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "name": "Nuke", "rounds": [13, 1] }));
        check(v, "after the series is decided");

        let mut v = base();
        v["matches"][0]["team_a"] = serde_json::json!({ "match": "gf", "outcome": "winner" });
        check(v, "not listed before it");

        let mut v = base();
        v["matches"][0]["extra"] = 1.into();
        check(v, "unknown field");
    }
}
