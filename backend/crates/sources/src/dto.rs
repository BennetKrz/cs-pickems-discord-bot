//! Source-neutral DTOs that every adapter returns.
//!
//! IDs are the source's external IDs; canonical IDs never appear here. A/B
//! values (score, rounds, winners, feeds) are in the **source's** team order;
//! the reconciler flips them to the canonical orientation. Every field a
//! source may not know is optional: `None` means "this source has no value",
//! so a lower-precedence source can fill it.
//!
//! The DTOs are stored as JSON in `source_records` and read back for merging
//! and replay, so changes must stay backward-compatible (add optional fields
//! only) or bump [`DTO_VERSION`].

use chrono::{DateTime, Utc};
use domain::{
    BracketSide, EventStatus, EventTier, FeedOutcome, MapStatus, MatchFormat, MatchStatus, Side,
    StageKind,
};
use serde::{Deserialize, Serialize};

/// Version of the stored DTO JSON (`source_records.payload.v`).
pub const DTO_VERSION: u32 = 1;

/// A tournament.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcEvent {
    pub ext_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<EventTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub begin_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<EventStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<SrcStage>,
}

/// A stage of an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcStage {
    pub ext_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<StageKind>,
    /// Sort position within the event.
    #[serde(default)]
    pub position: i64,
}

/// A team as one source knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcTeam {
    pub ext_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acronym: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
}

/// A series score or map score: `(team A, team B)` in source order.
pub type Pair<T> = (T, T);

/// One match (series).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcMatch {
    pub ext_id: String,
    pub event_ext_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_ext_id: Option<String>,
    /// `None` while TBD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_a: Option<SrcTeam>,
    /// `None` while TBD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_b: Option<SrcTeam>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<MatchFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<MatchStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// Maps won, in source order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<Pair<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<Side>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maps: Vec<SrcMap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bracket: Option<SrcBracket>,
}

/// One map of a match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcMap {
    /// 1–5.
    pub number: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<MapStatus>,
    /// Rounds won, in source order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<Pair<u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<Side>,
}

/// Bracket position and the matches feeding this one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcBracket {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<BracketSide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_index: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_index: Option<i64>,
    /// Feeds the source's team A slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feeds_a: Option<SrcFeed>,
    /// Feeds the source's team B slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feeds_b: Option<SrcFeed>,
}

/// A bracket slot fed by the winner or loser of another match of the same
/// source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcFeed {
    pub match_ext_id: String,
    pub outcome: FeedOutcome,
}

impl SrcTeam {
    /// A team with only a name, for tests and simple sources.
    pub fn named(ext_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            ext_id: ext_id.into(),
            name: name.into(),
            acronym: None,
            logo_url: None,
            country: None,
        }
    }
}

impl SrcMatch {
    /// A match with only IDs set; fill the rest with struct update syntax.
    pub fn new(ext_id: impl Into<String>, event_ext_id: impl Into<String>) -> Self {
        Self {
            ext_id: ext_id.into(),
            event_ext_id: event_ext_id.into(),
            stage_ext_id: None,
            team_a: None,
            team_b: None,
            format: None,
            status: None,
            scheduled_at: None,
            started_at: None,
            ended_at: None,
            score: None,
            winner: None,
            maps: Vec::new(),
            bracket: None,
        }
    }
}

impl SrcEvent {
    /// An event with only ID and name set.
    pub fn new(ext_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            ext_id: ext_id.into(),
            name: name.into(),
            tier: None,
            begin_at: None,
            end_at: None,
            status: None,
            stages: Vec::new(),
        }
    }
}

impl SrcMap {
    /// A map with only its number set.
    pub fn new(number: u8) -> Self {
        Self {
            number,
            name: None,
            status: None,
            rounds: None,
            winner: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn match_round_trips_through_json() {
        let m = SrcMatch {
            stage_ext_id: Some("s1".into()),
            team_a: Some(SrcTeam::named("t1", "Alpha")),
            format: Some(MatchFormat::Bo3),
            status: Some(MatchStatus::Finished),
            scheduled_at: Some(Utc.with_ymd_and_hms(2026, 11, 20, 18, 0, 0).unwrap()),
            score: Some((2, 1)),
            winner: Some(Side::A),
            maps: vec![SrcMap {
                name: Some("Inferno".into()),
                rounds: Some((13, 11)),
                winner: Some(Side::A),
                ..SrcMap::new(1)
            }],
            bracket: Some(SrcBracket {
                side: Some(BracketSide::Upper),
                feeds_a: Some(SrcFeed {
                    match_ext_id: "m0".into(),
                    outcome: FeedOutcome::Winner,
                }),
                ..SrcBracket::default()
            }),
            ..SrcMatch::new("m1", "e1")
        };
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["status"], "finished");
        assert_eq!(json["score"], serde_json::json!([2, 1]));
        assert!(json.get("team_b").is_none(), "None fields are omitted");
        let back: SrcMatch = serde_json::from_value(json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn missing_optional_fields_default_to_none() {
        let m: SrcMatch = serde_json::from_str(r#"{"ext_id":"m1","event_ext_id":"e1"}"#).unwrap();
        assert_eq!(m, SrcMatch::new("m1", "e1"));
    }
}
