//! PandaScore JSON → source-neutral DTOs. Pure functions, no I/O.
//!
//! Mapping (see `docs/adr/0001-pandascore-coverage.md`):
//! - a PandaScore *serie* is our event, its *tournaments* are our stages;
//! - `opponents[0]` is team A, `opponents[1]` team B; scores are matched by
//!   `team_id` because `results` may list the teams in the other order;
//! - map names and round scores are not in the free plan, so they are never
//!   set; map winners are;
//! - a canceled match (including a walkover) has no score and no winner.

use std::collections::HashMap;
use std::str::FromStr;

use domain::{
    BracketSide, EventStatus, EventTier, FeedOutcome, MapStatus, MatchFormat, MatchStatus, Side,
    StageKind,
};
use serde_json::Value;

use super::model::{PsMatch, PsPrevious, PsSerie, PsTeam, PsTournament};
use crate::{
    SourceError, SourceResult, SrcBracket, SrcEvent, SrcFeed, SrcMap, SrcMatch, SrcStage, SrcTeam,
};

/// PandaScore's videogame slug for Counter-Strike (CS2 included).
pub const CS_VIDEOGAME_SLUG: &str = "cs-go";

pub fn decode_match(raw: &Value) -> SourceResult<PsMatch> {
    serde_json::from_value(raw.clone()).map_err(|e| SourceError::Decode(format!("match: {e}")))
}

pub fn decode_serie(raw: &Value) -> SourceResult<PsSerie> {
    serde_json::from_value(raw.clone()).map_err(|e| SourceError::Decode(format!("serie: {e}")))
}

/// Whether a serie belongs to Counter-Strike. A serie without videogame
/// info (e.g. from a `/csgo/...` route) counts as CS.
pub fn is_cs(serie: &PsSerie) -> bool {
    serie
        .videogame
        .as_ref()
        .and_then(|v| v.slug.as_deref())
        .is_none_or(|slug| slug == CS_VIDEOGAME_SLUG)
}

/// A serie as an event, with its tournaments as stages.
pub fn serie_to_event(s: &PsSerie) -> SrcEvent {
    let league = s.league.as_ref().and_then(|l| l.name.as_deref());
    let name = [league, s.full_name.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let name = if !name.is_empty() {
        name
    } else {
        [s.name.as_deref(), s.slug.as_deref()]
            .into_iter()
            .flatten()
            .find(|n| !n.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Serie {}", s.id))
    };

    let mut tournaments: Vec<&PsTournament> = s.tournaments.iter().collect();
    tournaments.sort_by_key(|t| (t.begin_at, t.id));
    let stages = tournaments
        .iter()
        .enumerate()
        .map(|(i, t)| SrcStage {
            ext_id: t.id.to_string(),
            name: t
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| format!("Stage {}", i + 1)),
            kind: stage_kind(t.name.as_deref()),
            position: i as i64,
        })
        .collect();

    SrcEvent {
        ext_id: s.id.to_string(),
        name,
        tier: best_tier(&s.tournaments),
        begin_at: s.begin_at,
        end_at: s.end_at,
        // A serie has no status of its own; only a winner says it is over.
        status: s.winner_id.map(|_| EventStatus::Finished),
        stages,
    }
}

/// The best tier among the tournaments (`s` is best). `None` if no
/// tournament has a known tier, so a lower-precedence source can fill it.
fn best_tier(tournaments: &[PsTournament]) -> Option<EventTier> {
    const ORDER: [EventTier; 5] = [
        EventTier::S,
        EventTier::A,
        EventTier::B,
        EventTier::C,
        EventTier::D,
    ];
    tournaments
        .iter()
        .filter_map(|t| t.tier.as_deref())
        .filter_map(|t| EventTier::from_str(&t.to_ascii_lowercase()).ok())
        .filter_map(|t| ORDER.iter().position(|o| *o == t))
        .min()
        .map(|i| ORDER[i])
}

fn stage_kind(name: Option<&str>) -> Option<StageKind> {
    let name = name?.to_ascii_lowercase();
    if name.contains("swiss") {
        Some(StageKind::Swiss)
    } else if name.contains("group") {
        Some(StageKind::Groups)
    } else {
        // PandaScore does not say single or double elimination.
        None
    }
}

pub fn match_status(s: &str) -> Option<MatchStatus> {
    match s {
        "not_started" => Some(MatchStatus::Scheduled),
        "running" => Some(MatchStatus::Live),
        "finished" => Some(MatchStatus::Finished),
        "canceled" => Some(MatchStatus::Canceled),
        "postponed" => Some(MatchStatus::Postponed),
        _ => None,
    }
}

fn match_format(m: &PsMatch) -> Option<MatchFormat> {
    if m.match_type.as_deref() != Some("best_of") {
        return None;
    }
    match m.number_of_games? {
        1 => Some(MatchFormat::Bo1),
        3 => Some(MatchFormat::Bo3),
        5 => Some(MatchFormat::Bo5),
        _ => None,
    }
}

fn team(t: &PsTeam) -> SrcTeam {
    let non_empty = |s: &Option<String>| s.clone().filter(|v| !v.trim().is_empty());
    SrcTeam {
        ext_id: t.id.to_string(),
        name: t.name.clone(),
        acronym: non_empty(&t.acronym),
        logo_url: non_empty(&t.image_url),
        country: non_empty(&t.location),
    }
}

/// What the adapter knows about other matches of the same response, to
/// place bracket feeds in the right slot.
#[derive(Debug, Default)]
pub struct FeedLookup {
    matches: HashMap<u64, FeedInfo>,
}

#[derive(Debug, Clone, Copy)]
struct FeedInfo {
    teams: [Option<u64>; 2],
    winner: Option<u64>,
}

impl FeedLookup {
    pub fn new<'a>(matches: impl IntoIterator<Item = &'a PsMatch>) -> Self {
        let matches = matches
            .into_iter()
            .map(|m| {
                let teams = [0, 1].map(|i| m.opponents.get(i).map(|o| o.opponent.id));
                let finished = m.status.as_deref() == Some("finished");
                let winner = m.winner_id.filter(|_| finished);
                (m.id, FeedInfo { teams, winner })
            })
            .collect();
        Self { matches }
    }

    /// The team that comes out of `prev`, if that match is decided.
    fn team_from(&self, prev: &PsPrevious, outcome: FeedOutcome) -> Option<u64> {
        let info = self.matches.get(&prev.match_id)?;
        let winner = info.winner?;
        match outcome {
            FeedOutcome::Winner => Some(winner),
            FeedOutcome::Loser => info.teams.iter().flatten().copied().find(|t| *t != winner),
        }
    }
}

/// Converts a match. `None` if it cannot be tied to a serie.
pub fn convert_match(m: &PsMatch, lookup: &FeedLookup) -> Option<SrcMatch> {
    let Some(serie_id) = m
        .serie_id
        .or_else(|| m.tournament.as_ref().and_then(|t| t.serie_id))
    else {
        tracing::warn!(match_id = m.id, "PandaScore match without serie; skipped");
        return None;
    };

    let status = m.status.as_deref().and_then(|s| {
        let mapped = match_status(s);
        if mapped.is_none() {
            tracing::warn!(
                match_id = m.id,
                status = s,
                "unknown PandaScore match status"
            );
        }
        mapped
    });
    let format = match_format(m);
    let team_a = m.opponents.first().map(|o| &o.opponent);
    let team_b = m.opponents.get(1).map(|o| &o.opponent);
    let ids = (team_a.map(|t| t.id), team_b.map(|t| t.id));
    let side_of = |id: Option<u64>| -> Option<Side> {
        let id = id?;
        if Some(id) == ids.0 {
            Some(Side::A)
        } else if Some(id) == ids.1 {
            Some(Side::B)
        } else {
            None
        }
    };

    let in_play = matches!(status, Some(MatchStatus::Live | MatchStatus::Finished));
    let score = match ids {
        (Some(a), Some(b)) if in_play => {
            let of = |id: u64| {
                m.results
                    .iter()
                    .find(|r| r.team_id == Some(id))
                    .and_then(|r| r.score)
                    .and_then(|s| u8::try_from(s).ok())
            };
            of(a).zip(of(b))
        }
        _ => None,
    };
    let winner = if status == Some(MatchStatus::Finished) && m.draw != Some(true) {
        side_of(m.winner_id)
    } else {
        None
    };

    Some(SrcMatch {
        ext_id: m.id.to_string(),
        event_ext_id: serie_id.to_string(),
        stage_ext_id: m
            .tournament_id
            .or_else(|| m.tournament.as_ref().map(|t| t.id))
            .map(|id| id.to_string()),
        team_a: team_a.map(team),
        team_b: team_b.map(team),
        format,
        status,
        scheduled_at: m.scheduled_at,
        started_at: m.begin_at,
        ended_at: m.end_at,
        score,
        winner,
        maps: maps(m, status, format, side_of),
        bracket: m
            .previous_matches
            .as_ref()
            .map(|prev| bracket(m, prev, ids, lookup)),
    })
}

fn maps(
    m: &PsMatch,
    status: Option<MatchStatus>,
    format: Option<MatchFormat>,
    side_of: impl Fn(Option<u64>) -> Option<Side>,
) -> Vec<SrcMap> {
    let over = matches!(status, Some(MatchStatus::Finished | MatchStatus::Canceled));
    let mut maps: Vec<SrcMap> = m
        .games
        .iter()
        .filter_map(|g| {
            let number = g.position.filter(|p| (1..=5).contains(p))?;
            let (status, winner) = match g.status.as_deref() {
                Some("finished") => (
                    Some(MapStatus::Finished),
                    side_of(g.winner.as_ref().and_then(|w| w.id)),
                ),
                Some("running") => (Some(MapStatus::Live), None),
                Some("not_started") if over => (Some(MapStatus::NotPlayed), None),
                Some("not_started") => (Some(MapStatus::Upcoming), None),
                _ if over => (Some(MapStatus::NotPlayed), None),
                _ => (None, None),
            };
            // No `name` and no `rounds`: the free plan does not have them.
            Some(SrcMap {
                status,
                winner,
                ..SrcMap::new(number)
            })
        })
        .collect();
    maps.sort_by_key(|m| m.number);
    maps.dedup_by_key(|m| m.number);

    // PandaScore omits the unplayed maps of a finished series.
    if over {
        let max = format
            .map(MatchFormat::max_maps)
            .or(m.number_of_games)
            .unwrap_or(0)
            .min(5);
        for n in 1..=max {
            if !maps.iter().any(|m| m.number == n) {
                maps.push(SrcMap {
                    status: Some(MapStatus::NotPlayed),
                    ..SrcMap::new(n)
                });
            }
        }
        maps.sort_by_key(|m| m.number);
    }
    maps
}

fn bracket(
    m: &PsMatch,
    prev: &[PsPrevious],
    ids: (Option<u64>, Option<u64>),
    lookup: &FeedLookup,
) -> SrcBracket {
    let feeds: Vec<(SrcFeed, Option<u64>)> = prev
        .iter()
        .filter_map(|p| {
            let outcome = match p.kind.as_str() {
                "winner" => FeedOutcome::Winner,
                "loser" => FeedOutcome::Loser,
                other => {
                    tracing::warn!(match_id = m.id, kind = other, "unknown bracket link type");
                    return None;
                }
            };
            let feed = SrcFeed {
                match_ext_id: p.match_id.to_string(),
                outcome,
            };
            Some((feed, lookup.team_from(p, outcome)))
        })
        .collect();
    let (a, b) = ids;
    let is = |team: Option<u64>, slot: Option<u64>| team.is_some() && team == slot;

    let (feeds_a, feeds_b) = match feeds.as_slice() {
        [] => (None, None),
        [(f, t)] => {
            if is(*t, b) || (t.is_none() && a.is_some() && b.is_none()) {
                (None, Some(f.clone()))
            } else {
                (Some(f.clone()), None)
            }
        }
        [(f1, t1), (f2, t2), ..] => {
            if is(*t1, b) || is(*t2, a) {
                (Some(f2.clone()), Some(f1.clone()))
            } else {
                (Some(f1.clone()), Some(f2.clone()))
            }
        }
    };

    SrcBracket {
        side: bracket_side(m.name.as_deref()),
        round_index: None,
        slot_index: None,
        feeds_a,
        feeds_b,
    }
}

fn bracket_side(name: Option<&str>) -> Option<BracketSide> {
    let name = name?.to_ascii_lowercase();
    if name.contains("grand final") {
        Some(BracketSide::Final)
    } else if name.contains("lower bracket") {
        Some(BracketSide::Lower)
    } else if name.contains("upper bracket") {
        Some(BracketSide::Upper)
    } else {
        None
    }
}

/// Turns free text into a slug fragment for `search[slug]`:
/// "IEM Cologne" → `iem-cologne`.
pub fn slugify(query: &str) -> String {
    let mut out = String::new();
    for c in query.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn fixture(name: &str) -> Value {
        let text = match name {
            "bo1" => include_str!("../../fixtures/pandascore/match_bo1.json"),
            "bo3" => include_str!("../../fixtures/pandascore/match_bo3.json"),
            "bo5" => include_str!("../../fixtures/pandascore/match_bo5.json"),
            "walkover" => include_str!("../../fixtures/pandascore/match_canceled_walkover.json"),
            "tbd" => include_str!("../../fixtures/pandascore/match_tbd.json"),
            "postponed" => include_str!("../../fixtures/pandascore/match_postponed.json"),
            "running" => include_str!("../../fixtures/pandascore/match_running.json"),
            "serie" => include_str!("../../fixtures/pandascore/serie.json"),
            "search" => include_str!("../../fixtures/pandascore/series_search.json"),
            "serie_matches" => include_str!("../../fixtures/pandascore/serie_matches.json"),
            "brackets" => include_str!("../../fixtures/pandascore/brackets.json"),
            other => panic!("no fixture {other}"),
        };
        serde_json::from_str(text).unwrap()
    }

    fn convert(raw: &Value) -> SrcMatch {
        convert_match(&decode_match(raw).unwrap(), &FeedLookup::default()).unwrap()
    }

    fn map_states(m: &SrcMatch) -> Vec<(u8, Option<MapStatus>, Option<Side>)> {
        m.maps
            .iter()
            .map(|x| (x.number, x.status, x.winner))
            .collect()
    }

    fn assert_no_invented_map_data(m: &SrcMatch) {
        for map in &m.maps {
            assert_eq!(map.name, None, "free plan has no map names");
            assert_eq!(map.rounds, None, "free plan has no round scores");
        }
    }

    #[test]
    fn bo1_finished() {
        let m = convert(&fixture("bo1"));
        assert_eq!(m.ext_id, "1814117");
        assert_eq!(m.format, Some(MatchFormat::Bo1));
        assert_eq!(m.status, Some(MatchStatus::Finished));
        assert_eq!(m.score, Some((1, 0)));
        assert_eq!(m.winner, Some(Side::A));
        assert_eq!(
            map_states(&m),
            vec![(1, Some(MapStatus::Finished), Some(Side::A))]
        );
        assert_eq!(m.bracket, None, "match lists carry no bracket data");
        assert_no_invented_map_data(&m);
    }

    #[test]
    fn bo3_two_nil_pads_the_unplayed_map() {
        let m = convert(&fixture("bo3"));
        assert_eq!(m.event_ext_id, "11212");
        assert_eq!(m.stage_ext_id.as_deref(), Some("22466"));
        let a = m.team_a.as_ref().unwrap();
        assert_eq!((a.ext_id.as_str(), a.name.as_str()), ("129547", "Detonate"));
        assert_eq!(a.acronym.as_deref(), Some("DTN"));
        assert_eq!(a.country.as_deref(), Some("US"));
        let b = m.team_b.as_ref().unwrap();
        assert_eq!(b.acronym, None);
        assert_eq!(b.country, None, "empty location is no country");
        assert_eq!(m.format, Some(MatchFormat::Bo3));
        assert_eq!(m.score, Some((2, 0)));
        assert_eq!(m.winner, Some(Side::A));
        assert_eq!(
            m.scheduled_at,
            Some(Utc.with_ymd_and_hms(2026, 11, 15, 1, 45, 0).unwrap())
        );
        assert_eq!(
            map_states(&m),
            vec![
                (1, Some(MapStatus::Finished), Some(Side::A)),
                (2, Some(MapStatus::Finished), Some(Side::A)),
                (3, Some(MapStatus::NotPlayed), None),
            ]
        );
        assert_no_invented_map_data(&m);
    }

    #[test]
    fn bo5_score_follows_team_ids_not_result_order() {
        let raw = fixture("bo5");
        // Recorded with `results` in the same order as `opponents`; reverse
        // it, as PandaScore often does.
        let mut swapped = raw.clone();
        swapped["results"].as_array_mut().unwrap().reverse();
        for raw in [raw, swapped] {
            let m = convert(&raw);
            assert_eq!(m.format, Some(MatchFormat::Bo5));
            assert_eq!(m.team_a.as_ref().unwrap().ext_id, "137510");
            assert_eq!(m.score, Some((3, 0)));
            assert_eq!(m.winner, Some(Side::A));
            assert_eq!(
                map_states(&m),
                vec![
                    (1, Some(MapStatus::Finished), Some(Side::A)),
                    (2, Some(MapStatus::Finished), Some(Side::A)),
                    (3, Some(MapStatus::Finished), Some(Side::A)),
                    (4, Some(MapStatus::NotPlayed), None),
                    (5, Some(MapStatus::NotPlayed), None),
                ]
            );
            assert_no_invented_map_data(&m);
        }
    }

    #[test]
    fn walkover_is_canceled_without_winner_or_score() {
        let raw = fixture("walkover");
        assert_eq!(raw["forfeit"], true);
        assert!(raw["winner_id"].is_u64(), "PandaScore names a winner");
        let m = convert(&raw);
        assert_eq!(m.status, Some(MatchStatus::Canceled));
        assert_eq!(m.winner, None);
        assert_eq!(m.score, None);
        assert!(
            m.maps
                .iter()
                .all(|x| x.status == Some(MapStatus::NotPlayed) && x.winner.is_none())
        );
        assert_eq!(m.maps.len(), 3);
    }

    #[test]
    fn tbd_match_has_no_teams() {
        let m = convert(&fixture("tbd"));
        assert_eq!(m.status, Some(MatchStatus::Scheduled));
        assert_eq!((m.team_a.as_ref(), m.team_b.as_ref()), (None, None));
        assert_eq!(m.score, None);
        assert_eq!(m.winner, None);
        assert_eq!(
            map_states(&m),
            vec![
                (1, Some(MapStatus::Upcoming), None),
                (2, Some(MapStatus::Upcoming), None),
                (3, Some(MapStatus::Upcoming), None),
            ]
        );
    }

    #[test]
    fn one_known_team_stays_in_its_slot() {
        let mut raw = fixture("bo3");
        raw["status"] = "not_started".into();
        raw["opponents"].as_array_mut().unwrap().remove(0);
        let m = convert(&raw);
        assert_eq!(m.team_a.as_ref().unwrap().ext_id, "139204");
        assert_eq!(m.team_b, None);
        assert_eq!(m.score, None);
    }

    #[test]
    fn postponed_keeps_the_new_time() {
        let m = convert(&fixture("postponed"));
        assert_eq!(m.status, Some(MatchStatus::Postponed));
        assert_eq!(
            m.scheduled_at,
            Some(Utc.with_ymd_and_hms(2026, 11, 17, 0, 0, 0).unwrap())
        );
        assert_eq!(m.score, None);
    }

    #[test]
    fn running_match_has_live_map_and_partial_score() {
        let m = convert(&fixture("running"));
        assert_eq!(m.status, Some(MatchStatus::Live));
        assert_eq!(m.score, Some((1, 0)));
        assert_eq!(m.winner, None);
        assert_eq!(m.ended_at, None);
        assert_eq!(
            map_states(&m),
            vec![
                (1, Some(MapStatus::Finished), Some(Side::A)),
                (2, Some(MapStatus::Live), None),
                (3, Some(MapStatus::Upcoming), None),
            ]
        );
        assert_no_invented_map_data(&m);
    }

    #[test]
    fn draw_has_no_winner() {
        let mut raw = fixture("bo3");
        raw["draw"] = true.into();
        assert_eq!(convert(&raw).winner, None);
    }

    #[test]
    fn unknown_status_and_format_are_left_empty() {
        let mut raw = fixture("bo3");
        raw["status"] = "something_new".into();
        raw["match_type"] = "first_to".into();
        let m = convert(&raw);
        assert_eq!(m.status, None);
        assert_eq!(m.format, None);
        assert_eq!(m.score, None);
        assert_eq!(m.winner, None);
    }

    #[test]
    fn statuses_map_to_canonical() {
        assert_eq!(match_status("not_started"), Some(MatchStatus::Scheduled));
        assert_eq!(match_status("running"), Some(MatchStatus::Live));
        assert_eq!(match_status("finished"), Some(MatchStatus::Finished));
        assert_eq!(match_status("canceled"), Some(MatchStatus::Canceled));
        assert_eq!(match_status("postponed"), Some(MatchStatus::Postponed));
        assert_eq!(match_status("deleted"), None);
    }

    #[test]
    fn match_without_serie_is_skipped() {
        let mut raw = fixture("bo1");
        raw["serie_id"] = Value::Null;
        raw["tournament"]["serie_id"] = Value::Null;
        assert!(convert_match(&decode_match(&raw).unwrap(), &FeedLookup::default()).is_none());
    }

    fn bracket_matches() -> Vec<PsMatch> {
        fixture("brackets")
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                // Bracket responses carry no serie; the adapter merges them
                // into the match list, which does.
                let mut m = decode_match(v).unwrap();
                m.serie_id = Some(11212);
                m
            })
            .collect()
    }

    fn find(ms: &[PsMatch], id: u64) -> PsMatch {
        ms.iter().find(|m| m.id == id).unwrap().clone()
    }

    fn feed(id: &str, outcome: FeedOutcome) -> Option<SrcFeed> {
        Some(SrcFeed {
            match_ext_id: id.into(),
            outcome,
        })
    }

    #[test]
    fn bracket_feeds_follow_the_teams() {
        let ms = bracket_matches();
        let lookup = FeedLookup::new(&ms);
        // Grand final: VOCA (winner of 1814196) vs LAG (winner of 1814197).
        let mut gf = find(&ms, 1814198);
        let mut m = convert_match(&gf, &lookup).unwrap();
        let b = m.bracket.clone().unwrap();
        assert_eq!(b.side, Some(BracketSide::Final));
        assert_eq!(b.feeds_a, feed("1814196", FeedOutcome::Winner));
        assert_eq!(b.feeds_b, feed("1814197", FeedOutcome::Winner));

        // The same match with the teams listed the other way round.
        gf.opponents.reverse();
        m = convert_match(&gf, &lookup).unwrap();
        let b = m.bracket.unwrap();
        assert_eq!(b.feeds_a, feed("1814197", FeedOutcome::Winner));
        assert_eq!(b.feeds_b, feed("1814196", FeedOutcome::Winner));

        // Loser feeds: Arnie's Acolytes lost 1814194, Celestial lost 1814195.
        let mut decider = find(&ms, 1814325);
        decider.opponents.reverse();
        let b = convert_match(&decider, &lookup).unwrap().bracket.unwrap();
        assert_eq!(b.feeds_a, feed("1814195", FeedOutcome::Loser));
        assert_eq!(b.feeds_b, feed("1814194", FeedOutcome::Loser));

        // A first-round match: bracket data, but no feeds.
        let qf = convert_match(&find(&ms, 1814195), &lookup).unwrap();
        assert_eq!(qf.bracket, Some(SrcBracket::default()));
    }

    #[test]
    fn undecided_feeds_keep_their_order() {
        let ms = bracket_matches();
        let mut gf = find(&ms, 1814198);
        gf.opponents.clear();
        let b = convert_match(&gf, &FeedLookup::default())
            .unwrap()
            .bracket
            .unwrap();
        assert_eq!(b.feeds_a, feed("1814196", FeedOutcome::Winner));
        assert_eq!(b.feeds_b, feed("1814197", FeedOutcome::Winner));
    }

    #[test]
    fn bracket_side_from_name() {
        assert_eq!(
            bracket_side(Some("Upper Bracket Round 1 Match 4: A vs B")),
            Some(BracketSide::Upper)
        );
        assert_eq!(
            bracket_side(Some("Lower Bracket Final: TBD vs TBD")),
            Some(BracketSide::Lower)
        );
        assert_eq!(
            bracket_side(Some("Grand Final: A vs B")),
            Some(BracketSide::Final)
        );
        assert_eq!(bracket_side(Some("Quarterfinal 1: A vs B")), None);
    }

    #[test]
    fn serie_becomes_event_with_stages() {
        let s = decode_serie(&fixture("serie")).unwrap();
        assert!(is_cs(&s));
        let e = serie_to_event(&s);
        assert_eq!(e.ext_id, "11137");
        assert_eq!(e.name, "BLAST Rivals Fall 2026");
        assert_eq!(e.tier, Some(EventTier::S));
        assert_eq!(e.status, Some(EventStatus::Finished));
        assert_eq!(
            e.begin_at,
            Some(Utc.with_ymd_and_hms(2026, 11, 11, 2, 0, 0).unwrap())
        );
        let stages: Vec<_> = e
            .stages
            .iter()
            .map(|s| (s.ext_id.as_str(), s.name.as_str(), s.kind, s.position))
            .collect();
        assert_eq!(
            stages,
            vec![
                ("22269", "Group A", Some(StageKind::Groups), 0),
                ("22270", "Group B", Some(StageKind::Groups), 1),
                ("22271", "Playoffs", None, 2),
            ]
        );
    }

    #[test]
    fn best_tier_wins_and_missing_tier_is_none() {
        let mut s = decode_serie(&fixture("serie")).unwrap();
        s.tournaments[0].tier = Some("b".into());
        s.tournaments[1].tier = Some("a".into());
        s.tournaments[2].tier = Some("c".into());
        assert_eq!(serie_to_event(&s).tier, Some(EventTier::A));
        for t in &mut s.tournaments {
            t.tier = None;
        }
        assert_eq!(serie_to_event(&s).tier, None);
    }

    #[test]
    fn other_videogames_are_not_cs() {
        let mut raw = fixture("serie");
        raw["videogame"]["slug"] = "valorant".into();
        assert!(!is_cs(&decode_serie(&raw).unwrap()));
    }

    #[test]
    fn search_results_have_tier_dates_and_stages() {
        let events: Vec<SrcEvent> = fixture("search")
            .as_array()
            .unwrap()
            .iter()
            .map(|v| serie_to_event(&decode_serie(v).unwrap()))
            .collect();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].name, "BLAST Rivals Fall 2026");
        assert!(
            events
                .iter()
                .all(|e| e.tier == Some(EventTier::S) && e.begin_at.is_some())
        );
        assert!(events.iter().all(|e| e.stages.len() == 3));
    }

    #[test]
    fn every_recorded_match_converts() {
        for v in fixture("serie_matches").as_array().unwrap() {
            let m = convert(v);
            assert_eq!(m.event_ext_id, "11212");
            assert_eq!(m.status, Some(MatchStatus::Finished));
            assert!(m.winner.is_some(), "{} has a winner", m.ext_id);
            assert_no_invented_map_data(&m);
        }
    }

    #[test]
    fn slugify_queries() {
        assert_eq!(slugify("IEM Cologne"), "iem-cologne");
        assert_eq!(slugify("  BLAST  Rivals! "), "blast-rivals");
        assert_eq!(slugify("PGL Cluj-Napoca 2027"), "pgl-cluj-napoca-2027");
        assert_eq!(slugify("!!!"), "");
    }
}
