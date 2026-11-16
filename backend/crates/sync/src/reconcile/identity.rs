//! Cross-source identity rules, applied when a non-primary source reports an
//! entity it has no `external_refs` link for yet. An existing link (rule 1,
//! e.g. set by an admin) is checked by the caller first.

use chrono::{DateTime, Duration, Utc};
use domain::{Stage, Team};
use sources::{SrcStage, SrcTeam};

/// Rule 3: a match of another source is the same match if it is in the same
/// event, between the same two teams, and scheduled within this window.
pub const MATCH_WINDOW: Duration = Duration::hours(2);

/// Normalizes a name for comparison: lowercase, a leading `team ` removed,
/// then only letters and digits kept. `"Team Vitality"` → `"vitality"`,
/// `"Natus Vincere"` → `"natusvincere"`, `"G2 Esports"` → `"g2esports"`.
pub fn normalize(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let rest = lower.strip_prefix("team ").unwrap_or(&lower);
    rest.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn same(a: &str, b: &str) -> bool {
    let a = normalize(a);
    !a.is_empty() && a == normalize(b)
}

/// Rule 2: canonical teams with the same normalized name, or the same
/// normalized acronym (when both have one).
pub fn team_candidates<'a>(teams: &'a [Team], src: &SrcTeam) -> Vec<&'a Team> {
    teams
        .iter()
        .filter(|t| {
            same(&t.name, &src.name)
                || matches!((&t.acronym, &src.acronym), (Some(a), Some(b)) if same(a, b))
        })
        .collect()
}

/// Stages of the linked event with the same normalized name.
pub fn stage_candidates<'a>(stages: &'a [Stage], src: &SrcStage) -> Vec<&'a Stage> {
    stages.iter().filter(|s| same(&s.name, &src.name)).collect()
}

/// The time window for rule 3 around `at`.
pub fn match_window(at: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    (at - MATCH_WINDOW, at + MATCH_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::TeamId;

    fn team(id: i64, name: &str, acronym: Option<&str>) -> Team {
        Team {
            id: TeamId(id),
            name: name.into(),
            acronym: acronym.map(Into::into),
            logo_url: None,
            country: None,
        }
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize("Team Vitality"), "vitality");
        assert_eq!(normalize("  VITALITY "), "vitality");
        assert_eq!(normalize("Natus Vincere"), "natusvincere");
        assert_eq!(normalize("G2 Esports"), "g2esports");
        assert_eq!(normalize("Ninjas in Pyjamas"), "ninjasinpyjamas");
        assert_eq!(
            normalize("Team"),
            "team",
            "only a `team ` prefix is removed"
        );
        assert_eq!(normalize("Façade"), "façade");
        assert_eq!(normalize("!!!"), "");
    }

    #[test]
    fn teams_match_by_name_or_acronym() {
        let teams = vec![
            team(1, "Vitality", Some("VIT")),
            team(2, "Natus Vincere", Some("NAVI")),
            team(3, "FaZe Clan", None),
        ];
        let ids = |src: SrcTeam| -> Vec<i64> {
            team_candidates(&teams, &src)
                .iter()
                .map(|t| t.id.0)
                .collect()
        };
        assert_eq!(ids(SrcTeam::named("x", "Team Vitality")), vec![1]);
        assert_eq!(
            ids(SrcTeam {
                acronym: Some("navi".into()),
                ..SrcTeam::named("x", "NaVi Esports")
            }),
            vec![2]
        );
        assert_eq!(ids(SrcTeam::named("x", "faze-clan")), vec![3]);
        assert!(
            ids(SrcTeam::named("x", "FaZe")).is_empty(),
            "no fuzzy match"
        );
        assert!(ids(SrcTeam::named("x", "???")).is_empty());
    }

    #[test]
    fn several_candidates_are_all_returned() {
        let teams = vec![team(1, "Liquid", None), team(2, "Team Liquid", None)];
        assert_eq!(
            team_candidates(&teams, &SrcTeam::named("x", "Liquid")).len(),
            2
        );
    }
}
