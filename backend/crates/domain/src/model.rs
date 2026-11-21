//! The canonical data model: enums, IDs and entities shared by all crates.
//!
//! Scores are always stored relative to the match's team A/B orientation,
//! which is fixed when the match is first created (see
//! `docs/architecture.md`, key invariants).

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};

/// A string did not match any value of a domain enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownValue {
    /// Name of the enum, e.g. `MatchFormat`.
    pub kind: &'static str,
    pub value: String,
}

impl fmt::Display for UnknownValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown {} value `{}`", self.kind, self.value)
    }
}

impl std::error::Error for UnknownValue {}

/// Defines a fieldless enum with a fixed lowercase string form, used in the
/// database and the API.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $s:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant ),+
        }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The stable string form.
            pub fn as_str(self) -> &'static str {
                match self {
                    $( $name::$variant => $s ),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownValue;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $( $s => Ok($name::$variant), )+
                    _ => Err(UnknownValue {
                        kind: stringify!($name),
                        value: s.to_owned(),
                    }),
                }
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

string_enum! {
    /// Tournament tier as reported by the source.
    EventTier {
        S => "s",
        A => "a",
        B => "b",
        C => "c",
        D => "d",
        Unknown => "unknown",
    }
}

string_enum! {
    EventStatus {
        Upcoming => "upcoming",
        Running => "running",
        Finished => "finished",
        Canceled => "canceled",
    }
}

string_enum! {
    StageKind {
        Swiss => "swiss",
        Groups => "groups",
        SingleElim => "single_elim",
        DoubleElim => "double_elim",
        Other => "other",
    }
}

string_enum! {
    /// Series length.
    MatchFormat {
        Bo1 => "bo1",
        Bo3 => "bo3",
        Bo5 => "bo5",
    }
}

impl MatchFormat {
    /// Highest map number that can be played: 1, 3 or 5.
    pub fn max_maps(self) -> u8 {
        match self {
            MatchFormat::Bo1 => 1,
            MatchFormat::Bo3 => 3,
            MatchFormat::Bo5 => 5,
        }
    }

    /// Maps a team must win to win the series: 1, 2 or 3.
    pub fn wins_needed(self) -> u8 {
        self.max_maps() / 2 + 1
    }
}

string_enum! {
    MatchStatus {
        Scheduled => "scheduled",
        Live => "live",
        Finished => "finished",
        Canceled => "canceled",
        Postponed => "postponed",
    }
}

string_enum! {
    MapStatus {
        Upcoming => "upcoming",
        Live => "live",
        Finished => "finished",
        /// The series was decided before this map, e.g. map 3 after a 2:0.
        NotPlayed => "not_played",
    }
}

string_enum! {
    /// How settled a pick's points are (T11), stored in `pick_results.state`.
    ResultState {
        /// The match is live: finished maps count, the rest may still move.
        Provisional => "provisional",
        /// The match is finished. Parts still waiting for data (missing
        /// round scores) are counted in `pending_parts`.
        Final => "final",
        /// Canceled, forfeited or voided by an admin: 0 points, and the pick
        /// does not count for accuracy.
        Void => "void",
    }
}

string_enum! {
    /// Side of a double-elimination bracket; `Final` is the grand final.
    BracketSide {
        Upper => "upper",
        Lower => "lower",
        Final => "final",
    }
}

string_enum! {
    /// Which result of a previous match feeds a bracket slot.
    FeedOutcome {
        Winner => "winner",
        Loser => "loser",
    }
}

string_enum! {
    /// One of the two teams of a match, relative to some A/B orientation.
    Side {
        A => "a",
        B => "b",
    }
}

impl Side {
    /// The other side.
    pub fn other(self) -> Side {
        match self {
            Side::A => Side::B,
            Side::B => Side::A,
        }
    }
}

string_enum! {
    /// Canonical entities that can carry external IDs.
    EntityType {
        Team => "team",
        Event => "event",
        Stage => "stage",
        Match => "match",
    }
}

string_enum! {
    /// Why an entity a source reported is waiting in the unlinked queue
    /// instead of being linked to a canonical entity.
    UnlinkedReason {
        /// No canonical entity matched the identity rules.
        NoMatch => "no_match",
        /// More than one canonical entity matched.
        Ambiguous => "ambiguous",
        /// An entity it depends on (its event or a team) is not linked yet.
        Dependency => "dependency",
    }
}

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(TeamId);
id_type!(EventId);
id_type!(StageId);
id_type!(MatchId);
id_type!(PickId);

/// A team. `acronym`, `logo_url` and `country` are optional in every source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Team {
    pub id: TeamId,
    pub name: String,
    pub acronym: Option<String>,
    pub logo_url: Option<String>,
    pub country: Option<String>,
}

/// A team before it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTeam {
    pub name: String,
    pub acronym: Option<String>,
    pub logo_url: Option<String>,
    pub country: Option<String>,
}

/// A tournament.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: EventId,
    pub name: String,
    pub tier: EventTier,
    pub begin_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub status: EventStatus,
}

/// An event before it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    pub name: String,
    pub tier: EventTier,
    pub begin_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub status: EventStatus,
}

/// A stage of an event (Swiss, groups, playoffs, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub id: StageId,
    pub event_id: EventId,
    pub name: String,
    pub kind: StageKind,
    /// Sort position within the event, ascending.
    pub position: i64,
}

/// A stage before it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewStage {
    pub event_id: EventId,
    pub name: String,
    pub kind: StageKind,
    pub position: i64,
}

/// A bracket slot fed by the winner or loser of an earlier match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feed {
    pub match_id: MatchId,
    pub outcome: FeedOutcome,
}

/// Position of a match in a bracket. All fields are optional because Swiss
/// and group matches have no bracket position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BracketPos {
    pub side: Option<BracketSide>,
    pub round_index: Option<i64>,
    pub slot_index: Option<i64>,
    pub feeds_a_from: Option<Feed>,
    pub feeds_b_from: Option<Feed>,
}

/// The mutable fields of a match, shared by [`Match`] and [`NewMatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchFields {
    pub event_id: EventId,
    pub stage_id: Option<StageId>,
    /// `None` while the team is TBD.
    pub team_a_id: Option<TeamId>,
    /// `None` while the team is TBD.
    pub team_b_id: Option<TeamId>,
    pub format: MatchFormat,
    pub status: MatchStatus,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    pub locked: bool,
    /// An admin voided the match (T05): no points for anyone, picks are
    /// kept. Only admin actions change it; the reconciler carries it over.
    pub voided: bool,
    pub maps_won_a: u8,
    pub maps_won_b: u8,
    pub winner_team_id: Option<TeamId>,
    pub bracket: BracketPos,
}

impl MatchFields {
    /// A scheduled match with no score, no lock, no void and no bracket
    /// position.
    pub fn scheduled(
        event_id: EventId,
        team_a_id: Option<TeamId>,
        team_b_id: Option<TeamId>,
        format: MatchFormat,
        scheduled_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            event_id,
            stage_id: None,
            team_a_id,
            team_b_id,
            format,
            status: MatchStatus::Scheduled,
            scheduled_at,
            started_at: None,
            ended_at: None,
            lock_at: scheduled_at,
            locked: false,
            voided: false,
            maps_won_a: 0,
            maps_won_b: 0,
            winner_team_id: None,
            bracket: BracketPos::default(),
        }
    }
}

/// A match before it is stored.
pub type NewMatch = MatchFields;

/// A series between team A and team B.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub id: MatchId,
    pub fields: MatchFields,
}

/// One map of a match, keyed by its number (1–5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchMap {
    pub match_id: MatchId,
    pub number: u8,
    /// `None` until the veto is known.
    pub map_name: Option<String>,
    pub status: MapStatus,
    pub rounds_a: Option<u16>,
    pub rounds_b: Option<u16>,
    pub winner_team_id: Option<TeamId>,
}

/// A predicted series score, relative to the match's team A/B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeriesScore {
    pub a: u8,
    pub b: u8,
}

/// The prediction for one map of a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapPick {
    pub map_number: u8,
    pub winner_team_id: TeamId,
    /// Exact regulation score; `None` for a winner-only or overtime pick.
    pub rounds: Option<(u16, u16)>,
    /// The winner wins in overtime. Excludes `rounds`.
    pub overtime: bool,
}

/// The editable parts of a pick. Every part is optional; validation against
/// the match format is part of the pick rules (T10).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickParts {
    pub winner_team_id: Option<TeamId>,
    pub series: Option<SeriesScore>,
    /// Ordered by map number.
    pub maps: Vec<MapPick>,
}

/// One member's prediction for one match in one guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub id: PickId,
    pub guild_id: String,
    pub user_id: String,
    pub match_id: MatchId,
    pub parts: PickParts,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(all: &[T])
    where
        T: Copy + PartialEq + fmt::Debug + FromStr<Err = UnknownValue> + fmt::Display,
    {
        for v in all {
            assert_eq!(v.to_string().parse::<T>(), Ok(*v));
        }
    }

    #[test]
    fn enums_round_trip_through_strings() {
        round_trip(EventTier::ALL);
        round_trip(EventStatus::ALL);
        round_trip(StageKind::ALL);
        round_trip(MatchFormat::ALL);
        round_trip(MatchStatus::ALL);
        round_trip(MapStatus::ALL);
        round_trip(BracketSide::ALL);
        round_trip(FeedOutcome::ALL);
        round_trip(EntityType::ALL);
        round_trip(Side::ALL);
        round_trip(UnlinkedReason::ALL);
        round_trip(ResultState::ALL);
    }

    #[test]
    fn enums_serialize_as_their_string_form() {
        assert_eq!(
            serde_json::to_string(&StageKind::DoubleElim).unwrap(),
            "\"double_elim\""
        );
        assert_eq!(
            serde_json::from_str::<MatchStatus>("\"live\"").unwrap(),
            MatchStatus::Live
        );
        let err = serde_json::from_str::<MatchFormat>("\"bo7\"").unwrap_err();
        assert!(err.to_string().contains("unknown MatchFormat value `bo7`"));
    }

    #[test]
    fn string_forms_match_the_schema() {
        assert_eq!(StageKind::DoubleElim.as_str(), "double_elim");
        assert_eq!(MapStatus::NotPlayed.as_str(), "not_played");
        assert_eq!(EventTier::Unknown.as_str(), "unknown");
    }

    #[test]
    fn unknown_values_are_rejected() {
        let err = "bo7".parse::<MatchFormat>().unwrap_err();
        assert_eq!(err.kind, "MatchFormat");
        assert_eq!(err.to_string(), "unknown MatchFormat value `bo7`");
        assert!("Live".parse::<MatchStatus>().is_err(), "case-sensitive");
        assert!("".parse::<EventTier>().is_err());
    }

    #[test]
    fn format_limits() {
        let limits: Vec<_> = MatchFormat::ALL
            .iter()
            .map(|f| (f.max_maps(), f.wins_needed()))
            .collect();
        assert_eq!(limits, vec![(1, 1), (3, 2), (5, 3)]);
    }

    #[test]
    fn scheduled_match_locks_at_its_start() {
        let at = DateTime::from_timestamp(1_800_000_000, 0);
        let m = MatchFields::scheduled(EventId(1), None, None, MatchFormat::Bo3, at);
        assert_eq!(m.lock_at, at);
        assert_eq!(m.status, MatchStatus::Scheduled);
        assert!(!m.locked);
        assert!(!m.voided);
    }
}
