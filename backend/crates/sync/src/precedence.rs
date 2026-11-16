//! Field precedence: which source wins for each group of fields.
//!
//! Configured per field group, e.g. `schedule = ["manual", "pandascore"]`.
//! `manual` always comes first. Enabled sources a group does not list follow
//! the listed ones, in `ENABLED_SOURCES` order; sources that are not enabled
//! are ignored, also when their old records are still in the database.

use std::collections::BTreeMap;
use std::fmt;

/// The source name of admin overrides (T05).
pub const MANUAL: &str = "manual";

/// A group of canonical fields that one source sets as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FieldGroup {
    /// Event name, tier, dates and status.
    Event,
    /// Stage name, kind and position.
    Stage,
    /// Team name, acronym, logo and country.
    Team,
    /// The two teams of a match.
    Teams,
    /// Bo1/Bo3/Bo5.
    Format,
    /// `scheduled_at` (and so `lock_at`).
    Schedule,
    /// Match status, `started_at`, `ended_at`.
    Status,
    /// Maps won and the series winner.
    Series,
    /// Map names, per map.
    MapNames,
    /// Map status, rounds and winner, per map.
    MapRounds,
    /// Stage and bracket position, including feeds.
    Bracket,
}

impl FieldGroup {
    pub const ALL: [FieldGroup; 11] = [
        FieldGroup::Event,
        FieldGroup::Stage,
        FieldGroup::Team,
        FieldGroup::Teams,
        FieldGroup::Format,
        FieldGroup::Schedule,
        FieldGroup::Status,
        FieldGroup::Series,
        FieldGroup::MapNames,
        FieldGroup::MapRounds,
        FieldGroup::Bracket,
    ];

    /// The config key and provenance field name.
    pub fn as_str(self) -> &'static str {
        match self {
            FieldGroup::Event => "event",
            FieldGroup::Stage => "stage",
            FieldGroup::Team => "team",
            FieldGroup::Teams => "teams",
            FieldGroup::Format => "format",
            FieldGroup::Schedule => "schedule",
            FieldGroup::Status => "status",
            FieldGroup::Series => "series",
            FieldGroup::MapNames => "map_names",
            FieldGroup::MapRounds => "map_rounds",
            FieldGroup::Bracket => "bracket",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.as_str() == s)
    }
}

impl fmt::Display for FieldGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A precedence list that breaks the rules.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrecedenceError {
    #[error("precedence.{group}: `manual` must come first")]
    ManualNotFirst { group: FieldGroup },
    #[error("precedence.{group}: `{name}` is listed twice")]
    Duplicate { group: FieldGroup, name: String },
}

/// The resolved source order for every field group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Precedence {
    orders: BTreeMap<FieldGroup, Vec<String>>,
}

impl Precedence {
    /// Builds the order of every group from the enabled sources (in
    /// `ENABLED_SOURCES` order) and the configured lists. Listed sources
    /// that are not enabled are dropped.
    pub fn new(
        enabled: &[String],
        configured: &BTreeMap<FieldGroup, Vec<String>>,
    ) -> Result<Self, PrecedenceError> {
        let mut orders = BTreeMap::new();
        for group in FieldGroup::ALL {
            let listed = configured.get(&group).map(Vec::as_slice).unwrap_or(&[]);
            for (i, s) in listed.iter().enumerate() {
                if s == MANUAL && i != 0 {
                    return Err(PrecedenceError::ManualNotFirst { group });
                }
                if listed[..i].contains(s) {
                    return Err(PrecedenceError::Duplicate {
                        group,
                        name: s.clone(),
                    });
                }
            }
            let mut order = vec![MANUAL.to_owned()];
            for s in listed.iter().chain(enabled) {
                if !order.contains(s) && (enabled.contains(s)) {
                    order.push(s.clone());
                }
            }
            orders.insert(group, order);
        }
        Ok(Self { orders })
    }

    /// Default order for every group: `manual`, then `enabled`.
    pub fn from_enabled(enabled: &[String]) -> Self {
        Self::new(enabled, &BTreeMap::new()).expect("an empty config is always valid")
    }

    /// Sources for `group`, highest precedence first.
    pub fn order(&self, group: FieldGroup) -> &[String] {
        &self.orders[&group]
    }

    /// Position of `source` in `group`, or `None` if it does not take part.
    pub fn rank(&self, group: FieldGroup, source: &str) -> Option<usize> {
        self.order(group).iter().position(|s| s == source)
    }

    /// Every source that takes part in any group, `manual` first.
    pub fn sources(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for order in self.orders.values() {
            for s in order {
                if !out.contains(s) {
                    out.push(s.clone());
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn default_is_manual_then_enabled_order() {
        let p = Precedence::from_enabled(&s(&["alpha", "beta"]));
        for g in FieldGroup::ALL {
            assert_eq!(p.order(g), s(&["manual", "alpha", "beta"]));
        }
        assert_eq!(p.rank(FieldGroup::Schedule, "beta"), Some(2));
        assert_eq!(p.rank(FieldGroup::Schedule, "gamma"), None);
    }

    #[test]
    fn configured_groups_come_first_and_unlisted_follow() {
        let cfg = BTreeMap::from([
            (FieldGroup::MapRounds, s(&["beta"])),
            (FieldGroup::Schedule, s(&["manual", "alpha", "beta"])),
        ]);
        let p = Precedence::new(&s(&["alpha", "beta", "gamma"]), &cfg).unwrap();
        assert_eq!(
            p.order(FieldGroup::MapRounds),
            s(&["manual", "beta", "alpha", "gamma"])
        );
        assert_eq!(
            p.order(FieldGroup::Schedule),
            s(&["manual", "alpha", "beta", "gamma"])
        );
    }

    #[test]
    fn sources_that_are_not_enabled_are_dropped() {
        let cfg = BTreeMap::from([(FieldGroup::Series, s(&["pandascore", "fixture"]))]);
        let p = Precedence::new(&s(&["fixture"]), &cfg).unwrap();
        assert_eq!(p.order(FieldGroup::Series), s(&["manual", "fixture"]));
    }

    #[test]
    fn manual_must_be_first_and_no_duplicates() {
        let cfg = BTreeMap::from([(FieldGroup::Schedule, s(&["alpha", "manual"]))]);
        assert_eq!(
            Precedence::new(&s(&["alpha"]), &cfg),
            Err(PrecedenceError::ManualNotFirst {
                group: FieldGroup::Schedule
            })
        );
        let cfg = BTreeMap::from([(FieldGroup::Teams, s(&["alpha", "alpha"]))]);
        assert!(matches!(
            Precedence::new(&s(&["alpha"]), &cfg),
            Err(PrecedenceError::Duplicate { .. })
        ));
    }

    #[test]
    fn group_names_round_trip() {
        for g in FieldGroup::ALL {
            assert_eq!(FieldGroup::parse(g.as_str()), Some(g));
        }
        assert_eq!(FieldGroup::parse("map_rounds"), Some(FieldGroup::MapRounds));
        assert_eq!(FieldGroup::parse("nope"), None);
    }
}
