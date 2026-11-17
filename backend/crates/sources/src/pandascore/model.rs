//! The parts of PandaScore's JSON the adapter reads.
//!
//! Deliberately lenient: almost everything is optional and unknown fields
//! are ignored, so a new or missing field never breaks a sync. Only IDs are
//! required.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// A match (`/csgo/matches`, `/tournaments/{id}/brackets`).
#[derive(Debug, Clone, Deserialize)]
pub struct PsMatch {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub match_type: Option<String>,
    #[serde(default)]
    pub number_of_games: Option<u8>,
    #[serde(default)]
    pub scheduled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub begin_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub end_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub opponents: Vec<PsOpponent>,
    #[serde(default)]
    pub results: Vec<PsResult>,
    #[serde(default)]
    pub games: Vec<PsGame>,
    #[serde(default)]
    pub winner_id: Option<u64>,
    #[serde(default)]
    pub draw: Option<bool>,
    #[serde(default)]
    pub serie_id: Option<u64>,
    #[serde(default)]
    pub tournament_id: Option<u64>,
    #[serde(default)]
    pub tournament: Option<PsTournament>,
    /// Only on bracket responses. `None` = this response has no bracket
    /// data; `Some([])` = the match has no feeding matches.
    #[serde(default)]
    pub previous_matches: Option<Vec<PsPrevious>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PsOpponent {
    pub opponent: PsTeam,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PsTeam {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub acronym: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    /// ISO country code, sometimes empty.
    #[serde(default)]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PsResult {
    #[serde(default)]
    pub team_id: Option<u64>,
    #[serde(default)]
    pub score: Option<u32>,
}

/// A map as listed inside a match. The free plan has no map name or rounds
/// here; the game detail endpoints that have them return 403.
#[derive(Debug, Clone, Deserialize)]
pub struct PsGame {
    #[serde(default)]
    pub position: Option<u8>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub winner: Option<PsWinner>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PsWinner {
    #[serde(default)]
    pub id: Option<u64>,
}

/// A bracket link: this match's slot is fed by the winner or loser of
/// `match_id`.
#[derive(Debug, Clone, Deserialize)]
pub struct PsPrevious {
    #[serde(rename = "type")]
    pub kind: String,
    pub match_id: u64,
}

/// A PandaScore *tournament*: a stage of our event.
#[derive(Debug, Clone, Deserialize)]
pub struct PsTournament {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub begin_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub has_bracket: Option<bool>,
    #[serde(default)]
    pub serie_id: Option<u64>,
}

/// A PandaScore *serie*: our event (`/series/{id}`, `/csgo/series`).
#[derive(Debug, Clone, Deserialize)]
pub struct PsSerie {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub begin_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub end_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub winner_id: Option<u64>,
    #[serde(default)]
    pub league: Option<PsLeague>,
    #[serde(default)]
    pub tournaments: Vec<PsTournament>,
    #[serde(default)]
    pub videogame: Option<PsVideogame>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PsLeague {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PsVideogame {
    #[serde(default)]
    pub slug: Option<String>,
}
