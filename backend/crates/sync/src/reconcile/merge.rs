//! Pure merging of several sources' views into canonical values.
//!
//! Every view is first oriented to the canonical team A/B. Then, per field
//! group, the first source in precedence order that has a value wins.
//! Groups no source has a value for keep their current canonical value.

use std::collections::{BTreeMap, BTreeSet};

use domain::{
    BracketPos, EventId, EventStatus, EventTier, Feed, MapStatus, MatchFields, MatchFormat,
    MatchId, MatchMap, MatchStatus, NewEvent, NewTeam, Side, StageId, StageKind, TeamId,
};
use sources::{SrcEvent, SrcMap, SrcMatch, SrcStage, SrcTeam};

use crate::precedence::{FieldGroup, Precedence};

/// One source's view of a match, with its external references resolved to
/// canonical IDs. Everything A/B is in the **source's** order.
#[derive(Debug, Clone)]
pub struct MatchView {
    pub source: String,
    pub dto: SrcMatch,
    /// `None` if TBD or not linked.
    pub team_a: Option<TeamId>,
    pub team_b: Option<TeamId>,
    pub stage: Option<StageId>,
    pub feeds_a: Option<Feed>,
    pub feeds_b: Option<Feed>,
}

/// The merged canonical state of a match.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedMatch {
    pub fields: MatchFields,
    /// Only maps some source reported; other existing maps stay untouched.
    pub maps: Vec<MatchMap>,
    /// Field (`schedule`, `map_rounds.2`, …) → winning source.
    pub provenance: BTreeMap<String, String>,
}

/// A view turned into the canonical orientation.
struct Oriented<'a> {
    source: &'a str,
    dto: &'a SrcMatch,
    flipped: bool,
    team_a: Option<TeamId>,
    team_b: Option<TeamId>,
    stage: Option<StageId>,
    feeds_a: Option<Feed>,
    feeds_b: Option<Feed>,
}

impl Oriented<'_> {
    fn pair<T>(&self, (a, b): (T, T)) -> (T, T) {
        if self.flipped { (b, a) } else { (a, b) }
    }

    fn side(&self, s: Side) -> Side {
        if self.flipped { s.other() } else { s }
    }

    fn map(&self, number: u8) -> Option<&SrcMap> {
        self.dto.maps.iter().find(|m| m.number == number)
    }
}

fn hits(x: Option<TeamId>, y: Option<TeamId>) -> u8 {
    u8::from(x.is_some() && x == y)
}

/// Whether a view's teams are the reference teams in swapped order. A view
/// that shares no team with the reference is not flipped.
pub fn is_flipped(
    reference: (Option<TeamId>, Option<TeamId>),
    view: (Option<TeamId>, Option<TeamId>),
) -> bool {
    let straight = hits(view.0, reference.0) + hits(view.1, reference.1);
    let crossed = hits(view.0, reference.1) + hits(view.1, reference.0);
    crossed > straight
}

/// Anything that came from one source.
trait HasSource {
    fn source(&self) -> &str;
}

impl HasSource for MatchView {
    fn source(&self) -> &str {
        &self.source
    }
}

impl HasSource for Oriented<'_> {
    fn source(&self) -> &str {
        self.source
    }
}

impl<T> HasSource for (String, T) {
    fn source(&self) -> &str {
        &self.0
    }
}

/// Sorts items by the precedence of their source for `group` and drops
/// sources that do not take part.
fn ranked<'a, T: HasSource>(items: &'a [T], p: &Precedence, group: FieldGroup) -> Vec<&'a T> {
    let mut v: Vec<(usize, &T)> = items
        .iter()
        .filter_map(|it| p.rank(group, it.source()).map(|r| (r, it)))
        .collect();
    v.sort_by_key(|(r, _)| *r);
    v.into_iter().map(|(_, it)| it).collect()
}

/// The first ranked item for which `f` has a value, with its source.
fn first<'a, T: HasSource, R>(
    items: &[&'a T],
    f: impl Fn(&'a T) -> Option<R>,
) -> Option<(R, String)> {
    items
        .iter()
        .find_map(|it| f(it).map(|r| (r, it.source().to_owned())))
}

fn side_team(side: Side, a: Option<TeamId>, b: Option<TeamId>) -> Option<TeamId> {
    match side {
        Side::A => a,
        Side::B => b,
    }
}

/// Merges all views of a match. `current` is `None` for a new match, whose
/// orientation then comes from the highest-ranked view with teams.
pub fn merge_match(
    match_id: MatchId,
    event_id: EventId,
    current: Option<(&MatchFields, &[MatchMap])>,
    views: &[MatchView],
    p: &Precedence,
) -> MergedMatch {
    let cur = current.map(|(f, _)| f);
    let cur_maps = current.map(|(_, m)| m).unwrap_or(&[]);
    let mut prov = BTreeMap::new();

    // Orientation reference: the existing teams, else the top view's.
    let reference = match cur {
        Some(c) if c.team_a_id.is_some() || c.team_b_id.is_some() => (c.team_a_id, c.team_b_id),
        _ => ranked(views, p, FieldGroup::Teams)
            .into_iter()
            .find(|v| v.team_a.is_some() || v.team_b.is_some())
            .map(|v| (v.team_a, v.team_b))
            .unwrap_or((None, None)),
    };
    let oriented: Vec<Oriented<'_>> = views
        .iter()
        .map(|v| {
            let flipped = is_flipped(reference, (v.team_a, v.team_b));
            let (team_a, team_b) = if flipped {
                (v.team_b, v.team_a)
            } else {
                (v.team_a, v.team_b)
            };
            let (feeds_a, feeds_b) = if flipped {
                (v.feeds_b, v.feeds_a)
            } else {
                (v.feeds_a, v.feeds_b)
            };
            Oriented {
                source: &v.source,
                dto: &v.dto,
                flipped,
                team_a,
                team_b,
                stage: v.stage,
                feeds_a,
                feeds_b,
            }
        })
        .collect();
    let by = |g| ranked(&oriented, p, g);

    // Teams.
    let (team_a, team_b) = match first(&by(FieldGroup::Teams), |o| {
        (o.team_a.is_some() || o.team_b.is_some()).then_some((o.team_a, o.team_b))
    }) {
        Some((teams, s)) => {
            prov.insert(FieldGroup::Teams.to_string(), s);
            teams
        }
        None => cur
            .map(|c| (c.team_a_id, c.team_b_id))
            .unwrap_or((None, None)),
    };
    let team_b = if team_b.is_some() && team_b == team_a {
        None
    } else {
        team_b
    };
    let team = |s: Side| side_team(s, team_a, team_b);
    let is_team = |t: Option<TeamId>| t.is_some() && (t == team_a || t == team_b);

    // Format.
    let format = match first(&by(FieldGroup::Format), |o| o.dto.format) {
        Some((f, s)) => {
            prov.insert(FieldGroup::Format.to_string(), s);
            f
        }
        None => cur.map(|c| c.format).unwrap_or(MatchFormat::Bo3),
    };
    let wins = format.wins_needed();

    // Schedule.
    let scheduled_at = match first(&by(FieldGroup::Schedule), |o| o.dto.scheduled_at) {
        Some((t, s)) => {
            prov.insert(FieldGroup::Schedule.to_string(), s);
            Some(t)
        }
        None => cur.and_then(|c| c.scheduled_at),
    };

    // Status with its timestamps.
    let (status, started_at, ended_at) = match first(&by(FieldGroup::Status), |o| {
        o.dto
            .status
            .map(|st| (st, o.dto.started_at, o.dto.ended_at))
    }) {
        Some((v, s)) => {
            prov.insert(FieldGroup::Status.to_string(), s);
            v
        }
        None => cur
            .map(|c| (c.status, c.started_at, c.ended_at))
            .unwrap_or((MatchStatus::Scheduled, None, None)),
    };

    // Series score and winner. A score impossible for the format is ignored.
    let (maps_won_a, maps_won_b, winner_team_id) = match first(&by(FieldGroup::Series), |o| {
        let score = o
            .dto
            .score
            .map(|sc| o.pair(sc))
            .filter(|&(a, b)| a <= wins && b <= wins);
        let winner = o.dto.winner.map(|w| o.side(w));
        (score.is_some() || winner.is_some()).then_some((score, winner))
    }) {
        Some(((score, winner), s)) => {
            prov.insert(FieldGroup::Series.to_string(), s);
            let (a, b) = score.unwrap_or_else(|| {
                cur.map(|c| (c.maps_won_a.min(wins), c.maps_won_b.min(wins)))
                    .unwrap_or((0, 0))
            });
            let derived = match (a >= wins, b >= wins) {
                (true, false) => Some(Side::A),
                (false, true) => Some(Side::B),
                _ => None,
            };
            (a, b, winner.or(derived).and_then(team))
        }
        None => match cur {
            Some(c) => (
                c.maps_won_a.min(wins),
                c.maps_won_b.min(wins),
                c.winner_team_id.filter(|&w| is_team(Some(w))),
            ),
            None => (0, 0, None),
        },
    };

    // Stage and bracket position.
    let (stage_id, bracket) = match first(&by(FieldGroup::Bracket), |o| {
        (o.dto.stage_ext_id.is_some() || o.dto.bracket.is_some()).then_some(o)
    }) {
        Some((o, s)) => {
            prov.insert(FieldGroup::Bracket.to_string(), s);
            let b = o.dto.bracket.clone().unwrap_or_default();
            let not_self = |f: Option<Feed>| f.filter(|f| f.match_id != match_id);
            (
                o.stage,
                BracketPos {
                    side: b.side,
                    round_index: b.round_index,
                    slot_index: b.slot_index,
                    feeds_a_from: not_self(o.feeds_a),
                    feeds_b_from: not_self(o.feeds_b),
                },
            )
        }
        None => cur
            .map(|c| (c.stage_id, c.bracket))
            .unwrap_or((None, BracketPos::default())),
    };

    let locked = cur.is_some_and(|c| c.locked);
    // Admin state (T05); no source ever sets it.
    let voided = cur.is_some_and(|c| c.voided);
    let lock_at = if locked {
        cur.and_then(|c| c.lock_at)
    } else {
        scheduled_at
    };

    let fields = MatchFields {
        event_id,
        stage_id,
        team_a_id: team_a,
        team_b_id: team_b,
        format,
        status,
        scheduled_at,
        started_at,
        ended_at,
        lock_at,
        locked,
        voided,
        maps_won_a,
        maps_won_b,
        winner_team_id,
        bracket,
    };

    // Maps, per map number.
    let names_by = by(FieldGroup::MapNames);
    let rounds_by = by(FieldGroup::MapRounds);
    let numbers: BTreeSet<u8> = names_by
        .iter()
        .chain(&rounds_by)
        .flat_map(|o| o.dto.maps.iter().map(|m| m.number))
        .filter(|n| (1..=format.max_maps()).contains(n))
        .collect();
    let mut maps = Vec::new();
    for n in numbers {
        let cur_map = cur_maps.iter().find(|m| m.number == n);
        let map_name = match first(&names_by, |o| o.map(n).and_then(|m| m.name.clone())) {
            Some((name, s)) => {
                prov.insert(format!("{}.{n}", FieldGroup::MapNames), s);
                Some(name)
            }
            None => cur_map.and_then(|m| m.map_name.clone()),
        };
        let (status, rounds, winner_team_id) = match first(&rounds_by, |o| {
            o.map(n)
                .filter(|m| m.status.is_some() || m.rounds.is_some() || m.winner.is_some())
                .map(|m| {
                    (
                        m.status,
                        m.rounds.map(|r| o.pair(r)),
                        m.winner.map(|w| o.side(w)),
                    )
                })
        }) {
            Some(((status, rounds, winner), s)) => {
                prov.insert(format!("{}.{n}", FieldGroup::MapRounds), s);
                let status = status.unwrap_or(if winner.is_some() {
                    MapStatus::Finished
                } else {
                    cur_map.map_or(MapStatus::Upcoming, |m| m.status)
                });
                (status, rounds, winner.and_then(team))
            }
            None => match cur_map {
                Some(m) => (
                    m.status,
                    m.rounds_a.zip(m.rounds_b),
                    m.winner_team_id.filter(|&w| is_team(Some(w))),
                ),
                None => (MapStatus::Upcoming, None, None),
            },
        };
        maps.push(MatchMap {
            match_id,
            number: n,
            map_name,
            status,
            rounds_a: rounds.map(|r| r.0),
            rounds_b: rounds.map(|r| r.1),
            winner_team_id,
        });
    }

    MergedMatch {
        fields,
        maps,
        provenance: prov,
    }
}

/// Merges team views: name from the top source, other fields from the first
/// source that has them. `None` if no view takes part.
pub fn merge_team(views: &[(String, SrcTeam)], p: &Precedence) -> Option<(NewTeam, String)> {
    let r = ranked(views, p, FieldGroup::Team);
    let top = r.first()?;
    let fill = |f: fn(&SrcTeam) -> &Option<String>| r.iter().find_map(|v| f(&v.1).clone());
    Some((
        NewTeam {
            name: top.1.name.clone(),
            acronym: fill(|t| &t.acronym),
            logo_url: fill(|t| &t.logo_url),
            country: fill(|t| &t.country),
        },
        top.0.clone(),
    ))
}

/// Merges event views like [`merge_team`].
pub fn merge_event(views: &[(String, SrcEvent)], p: &Precedence) -> Option<(NewEvent, String)> {
    let r = ranked(views, p, FieldGroup::Event);
    let top = r.first()?;
    Some((
        NewEvent {
            name: top.1.name.clone(),
            tier: r
                .iter()
                .find_map(|v| v.1.tier)
                .unwrap_or(EventTier::Unknown),
            begin_at: r.iter().find_map(|v| v.1.begin_at),
            end_at: r.iter().find_map(|v| v.1.end_at),
            status: r
                .iter()
                .find_map(|v| v.1.status)
                .unwrap_or(EventStatus::Upcoming),
        },
        top.0.clone(),
    ))
}

/// Merged stage values: name, kind, position, and the winning source.
pub fn merge_stage(
    views: &[(String, SrcStage)],
    p: &Precedence,
) -> Option<(String, StageKind, i64, String)> {
    let r = ranked(views, p, FieldGroup::Stage);
    let top = r.first()?;
    Some((
        top.1.name.clone(),
        r.iter().find_map(|v| v.1.kind).unwrap_or(StageKind::Other),
        top.1.position,
        top.0.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};
    use domain::FeedOutcome;
    use sources::SrcBracket;

    const X: Option<TeamId> = Some(TeamId(10));
    const Y: Option<TeamId> = Some(TeamId(20));

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, h, 0, 0).unwrap()
    }

    fn prec(enabled: &[&str]) -> Precedence {
        Precedence::from_enabled(&enabled.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    fn view(source: &str, a: Option<TeamId>, b: Option<TeamId>, dto: SrcMatch) -> MatchView {
        MatchView {
            source: source.into(),
            dto,
            team_a: a,
            team_b: b,
            stage: None,
            feeds_a: None,
            feeds_b: None,
        }
    }

    fn finished_2_1() -> SrcMatch {
        SrcMatch {
            format: Some(MatchFormat::Bo3),
            status: Some(MatchStatus::Finished),
            scheduled_at: Some(at(18)),
            score: Some((2, 1)),
            winner: Some(Side::A),
            maps: vec![SrcMap {
                name: Some("Inferno".into()),
                rounds: Some((13, 7)),
                winner: Some(Side::A),
                ..SrcMap::new(1)
            }],
            ..SrcMatch::new("m", "e")
        }
    }

    #[test]
    fn flip_detection() {
        assert!(!is_flipped((X, Y), (X, Y)));
        assert!(is_flipped((X, Y), (Y, X)));
        assert!(is_flipped((X, None), (None, X)));
        assert!(is_flipped((X, None), (Y, X)));
        assert!(!is_flipped((X, Y), (None, None)));
        assert!(!is_flipped((None, None), (Y, X)));
    }

    #[test]
    fn swapped_view_is_flipped_into_canonical_order() {
        let current = MatchFields::scheduled(EventId(1), X, Y, MatchFormat::Bo3, Some(at(18)));
        // The source lists Y first; Y won 2:1 and map 1 13:7.
        let v = view("beta", Y, X, finished_2_1());
        let m = merge_match(
            MatchId(1),
            EventId(1),
            Some((&current, &[])),
            &[v],
            &prec(&["beta"]),
        );
        assert_eq!((m.fields.team_a_id, m.fields.team_b_id), (X, Y));
        assert_eq!((m.fields.maps_won_a, m.fields.maps_won_b), (1, 2));
        assert_eq!(m.fields.winner_team_id, Y);
        assert_eq!(
            (m.maps[0].rounds_a, m.maps[0].rounds_b),
            (Some(7), Some(13))
        );
        assert_eq!(m.maps[0].winner_team_id, Y);
        assert_eq!(m.maps[0].status, MapStatus::Finished);
    }

    #[test]
    fn new_match_takes_the_top_views_orientation() {
        let m = merge_match(
            MatchId(0),
            EventId(1),
            None,
            &[view("alpha", Y, X, finished_2_1())],
            &prec(&["alpha"]),
        );
        assert_eq!((m.fields.team_a_id, m.fields.team_b_id), (Y, X));
        assert_eq!((m.fields.maps_won_a, m.fields.maps_won_b), (2, 1));
    }

    #[test]
    fn tbd_slot_is_filled_without_moving_the_known_team() {
        let current = MatchFields::scheduled(EventId(1), None, X, MatchFormat::Bo1, Some(at(18)));
        let v = view("alpha", X, Y, SrcMatch::new("m", "e"));
        let m = merge_match(
            MatchId(1),
            EventId(1),
            Some((&current, &[])),
            &[v],
            &prec(&["alpha"]),
        );
        assert_eq!((m.fields.team_a_id, m.fields.team_b_id), (Y, X));
    }

    #[test]
    fn each_group_takes_the_first_source_with_a_value() {
        let alpha = SrcMatch {
            scheduled_at: Some(at(18)),
            maps: vec![SrcMap {
                rounds: Some((13, 10)),
                ..SrcMap::new(1)
            }],
            ..SrcMatch::new("a", "e")
        };
        let beta = SrcMatch {
            scheduled_at: Some(at(19)),
            format: Some(MatchFormat::Bo3),
            maps: vec![SrcMap {
                name: Some("Nuke".into()),
                rounds: Some((16, 14)),
                ..SrcMap::new(1)
            }],
            ..SrcMatch::new("b", "e")
        };
        let views = [view("alpha", X, Y, alpha), view("beta", X, Y, beta)];
        let m = merge_match(
            MatchId(1),
            EventId(1),
            None,
            &views,
            &prec(&["alpha", "beta"]),
        );
        assert_eq!(m.fields.scheduled_at, Some(at(18)));
        assert_eq!(m.fields.lock_at, Some(at(18)), "lock follows the schedule");
        assert_eq!(
            m.fields.format,
            MatchFormat::Bo3,
            "alpha has none; beta fills"
        );
        assert_eq!(m.maps[0].rounds_a, Some(13));
        assert_eq!(m.maps[0].map_name.as_deref(), Some("Nuke"));
        assert_eq!(m.provenance["schedule"], "alpha");
        assert_eq!(m.provenance["format"], "beta");
        assert_eq!(m.provenance["map_rounds.1"], "alpha");
        assert_eq!(m.provenance["map_names.1"], "beta");
        assert!(!m.provenance.contains_key("status"));
    }

    #[test]
    fn missing_groups_keep_current_values_and_locked_keeps_lock_at() {
        let mut current = MatchFields::scheduled(EventId(1), X, Y, MatchFormat::Bo5, Some(at(18)));
        current.locked = true;
        current.voided = true;
        current.status = MatchStatus::Live;
        let v = view(
            "alpha",
            X,
            Y,
            SrcMatch {
                scheduled_at: Some(at(20)),
                ..SrcMatch::new("m", "e")
            },
        );
        let m = merge_match(
            MatchId(1),
            EventId(1),
            Some((&current, &[])),
            &[v],
            &prec(&["alpha"]),
        );
        assert_eq!(m.fields.format, MatchFormat::Bo5);
        assert_eq!(m.fields.status, MatchStatus::Live);
        assert_eq!(m.fields.scheduled_at, Some(at(20)));
        assert_eq!(m.fields.lock_at, Some(at(18)));
        assert!(m.fields.locked);
        assert!(m.fields.voided, "admin state is carried over");
    }

    #[test]
    fn impossible_scores_and_maps_are_ignored_and_winner_is_derived() {
        let dto = SrcMatch {
            format: Some(MatchFormat::Bo1),
            score: Some((2, 0)),
            maps: vec![SrcMap::new(1), SrcMap::new(2)],
            ..SrcMatch::new("m", "e")
        };
        let other = SrcMatch {
            score: Some((0, 1)),
            ..SrcMatch::new("m2", "e")
        };
        let m = merge_match(
            MatchId(1),
            EventId(1),
            None,
            &[view("alpha", X, Y, dto), view("beta", X, Y, other)],
            &prec(&["alpha", "beta"]),
        );
        assert_eq!((m.fields.maps_won_a, m.fields.maps_won_b), (0, 1));
        assert_eq!(m.fields.winner_team_id, Y, "derived from 0:1 in a Bo1");
        assert_eq!(m.provenance["series"], "beta");
        assert_eq!(m.maps.len(), 1, "map 2 does not exist in a Bo1");
    }

    #[test]
    fn sources_outside_the_precedence_are_ignored() {
        let v = view(
            "old",
            X,
            Y,
            SrcMatch {
                scheduled_at: Some(at(9)),
                ..SrcMatch::new("m", "e")
            },
        );
        let current = MatchFields::scheduled(EventId(1), X, Y, MatchFormat::Bo3, Some(at(18)));
        let m = merge_match(
            MatchId(1),
            EventId(1),
            Some((&current, &[])),
            &[v],
            &prec(&["alpha"]),
        );
        assert_eq!(m.fields, current);
        assert!(m.provenance.is_empty());
    }

    #[test]
    fn feeds_follow_the_flip() {
        let current = MatchFields::scheduled(EventId(1), X, Y, MatchFormat::Bo3, None);
        let mut v = view(
            "beta",
            Y,
            X,
            SrcMatch {
                bracket: Some(SrcBracket::default()),
                ..SrcMatch::new("m", "e")
            },
        );
        let feed = Feed {
            match_id: MatchId(7),
            outcome: FeedOutcome::Winner,
        };
        v.feeds_a = Some(feed);
        let m = merge_match(
            MatchId(1),
            EventId(1),
            Some((&current, &[])),
            &[v],
            &prec(&["beta"]),
        );
        assert_eq!(m.fields.bracket.feeds_b_from, Some(feed));
        assert_eq!(m.fields.bracket.feeds_a_from, None);
    }

    #[test]
    fn team_and_event_fill_gaps_from_lower_sources() {
        let views = vec![
            ("beta".to_owned(), SrcTeam::named("b1", "Vitality")),
            (
                "alpha".to_owned(),
                SrcTeam {
                    acronym: Some("VIT".into()),
                    ..SrcTeam::named("a1", "Team Vitality")
                },
            ),
        ];
        let (t, s) = merge_team(&views, &prec(&["alpha", "beta"])).unwrap();
        assert_eq!(
            (t.name.as_str(), t.acronym.as_deref(), s.as_str()),
            ("Team Vitality", Some("VIT"), "alpha")
        );
        let (t, _) = merge_team(&views, &prec(&["beta", "alpha"])).unwrap();
        assert_eq!(
            (t.name.as_str(), t.acronym.as_deref()),
            ("Vitality", Some("VIT"))
        );
        assert!(merge_team(&views, &prec(&["gamma"])).is_none());

        let (e, _) = merge_event(
            &[("alpha".into(), SrcEvent::new("e", "Major"))],
            &prec(&["alpha"]),
        )
        .unwrap();
        assert_eq!(
            (e.tier, e.status),
            (EventTier::Unknown, EventStatus::Upcoming)
        );
    }
}
