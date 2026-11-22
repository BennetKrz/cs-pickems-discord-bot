//! Scoring rules (T11): how many points a pick earns against a result.
//!
//! [`score`] is a pure function of the pick, the match result and the
//! guild's [`ScoringRules`]. Settlement (`sync::settle`) recomputes it for
//! every pick of a match and overwrites `pick_results`, so corrections and
//! replays are safe. See `docs/tickets/T11-scoring-settlement.md` and
//! `docs/adr/0011-scoring-and-settlement.md`.
//!
//! Each part of a pick has its own outcome:
//!
//! | Part | Points by default |
//! | --- | --- |
//! | winner, correct | 3 |
//! | series score (Bo3/Bo5), exact | 3 |
//! | map winner, correct, per map | 1 |
//! | map score, exact regulation score or correct overtime winner, per map | 2 more |
//!
//! A part is `pending` while its data is missing; after the data deadline
//! (48 h after the match ended, decided by the caller) it becomes `void`.
//! A `void` part scores 0 and counts neither as hit nor as miss.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::{
    MapPick, MapStatus, Match, MatchFormat, MatchMap, MatchStatus, PickParts, ResultState,
    SeriesScore, TeamId,
};
use crate::picks::{MAP_MAX_LOSER_ROUNDS, MAP_WIN_ROUNDS};

/// Hours after the match ended until parts still waiting for data (round
/// scores, a missing map winner) become void.
pub const DATA_DEADLINE_HOURS: i64 = 48;

/// Highest value any part may be worth, to catch typos in a rules file.
pub const MAX_PART_POINTS: u32 = 100;

/// The points of each pick part. Stored per guild as JSON with a version
/// number; a missing field takes its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScoringRules {
    /// Correct match winner.
    pub winner: u32,
    /// Exact series score (Bo3/Bo5 only).
    pub series: u32,
    /// Correct map winner, per map.
    pub map_winner: u32,
    /// Exact regulation score or correct overtime winner, per map, on top
    /// of `map_winner`.
    pub map_score: u32,
}

impl Default for ScoringRules {
    fn default() -> Self {
        Self {
            winner: 3,
            series: 3,
            map_winner: 1,
            map_score: 2,
        }
    }
}

/// A rules file is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesError(pub String);

impl fmt::Display for RulesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RulesError {}

impl ScoringRules {
    /// Checks that no part is worth more than [`MAX_PART_POINTS`].
    pub fn validate(&self) -> Result<(), RulesError> {
        for (name, v) in [
            ("winner", self.winner),
            ("series", self.series),
            ("map_winner", self.map_winner),
            ("map_score", self.map_score),
        ] {
            if v > MAX_PART_POINTS {
                return Err(RulesError(format!(
                    "`{name}` is {v}; a part is worth at most {MAX_PART_POINTS} points"
                )));
            }
        }
        Ok(())
    }

    /// The most a single pick can earn for a match of `format`: 6, 15 and
    /// 21 with the default rules.
    pub fn max_points(&self, format: MatchFormat) -> u32 {
        let series = if format == MatchFormat::Bo1 {
            0
        } else {
            self.series
        };
        self.winner + series + u32::from(format.max_maps()) * (self.map_winner + self.map_score)
    }
}

// ---- results ------------------------------------------------------------------

/// Where a match stands, as far as scoring is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Not started (scheduled or postponed): nothing to score.
    Open,
    /// Running: finished maps score provisionally.
    Live,
    Finished,
    /// Canceled (also walkovers, ADR 0001) or voided by an admin.
    Void,
}

/// The result of one map, relative to the match's team A/B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapResult {
    pub number: u8,
    pub status: MapStatus,
    pub winner: Option<TeamId>,
    pub rounds: Option<(u16, u16)>,
}

/// A match result, source-neutral, as [`score`] needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    pub format: MatchFormat,
    pub team_a: Option<TeamId>,
    pub team_b: Option<TeamId>,
    pub phase: Phase,
    pub winner: Option<TeamId>,
    pub series: SeriesScore,
    pub maps: Vec<MapResult>,
    /// The data deadline has passed: parts still missing data become void
    /// instead of pending. Only matters for a finished match.
    pub deadline_passed: bool,
}

impl MatchResult {
    /// The result view of a stored match and its maps.
    pub fn from_match(m: &Match, maps: &[MatchMap], deadline_passed: bool) -> Self {
        let f = &m.fields;
        let phase = if f.voided {
            Phase::Void
        } else {
            match f.status {
                MatchStatus::Scheduled | MatchStatus::Postponed => Phase::Open,
                MatchStatus::Live => Phase::Live,
                MatchStatus::Finished => Phase::Finished,
                MatchStatus::Canceled => Phase::Void,
            }
        };
        Self {
            format: f.format,
            team_a: f.team_a_id,
            team_b: f.team_b_id,
            phase,
            winner: f.winner_team_id,
            series: SeriesScore {
                a: f.maps_won_a,
                b: f.maps_won_b,
            },
            maps: maps
                .iter()
                .map(|mm| MapResult {
                    number: mm.number,
                    status: mm.status,
                    winner: mm.winner_team_id,
                    rounds: mm.rounds_a.zip(mm.rounds_b),
                })
                .collect(),
            deadline_passed,
        }
    }

    /// Whether the series score shows a decided series.
    fn series_decided(&self) -> bool {
        let w = self.format.wins_needed();
        (self.series.a == w) != (self.series.b == w)
    }

    /// The match winner: the stored one, else the side the series score
    /// shows as decided.
    fn match_winner(&self) -> Option<TeamId> {
        if self.winner.is_some() {
            return self.winner;
        }
        if !self.series_decided() {
            return None;
        }
        if self.series.a > self.series.b {
            self.team_a
        } else {
            self.team_b
        }
    }

    /// The map with this number was never played: marked `not_played`, or
    /// beyond the maps of a decided, finished series.
    fn map_not_played(&self, number: u8) -> bool {
        let marked = self
            .maps
            .iter()
            .any(|m| m.number == number && m.status == MapStatus::NotPlayed);
        let beyond = self.phase == Phase::Finished
            && self.series_decided()
            && number > self.series.a + self.series.b;
        marked || beyond
    }
}

// ---- outcomes -----------------------------------------------------------------

/// The outcome of one part of a pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Hit,
    Miss,
    /// Waiting for the result or for missing data.
    Pending,
    /// Can't be scored: the match was voided, the map was never played, or
    /// the data never arrived. Not counted for accuracy.
    Void,
}

/// One scored part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartScore {
    pub outcome: Outcome,
    pub points: u32,
}

impl PartScore {
    fn new(outcome: Outcome, worth: u32) -> Self {
        let points = if outcome == Outcome::Hit { worth } else { 0 };
        Self { outcome, points }
    }
}

/// The scored parts of one map pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapScore {
    pub map_number: u8,
    pub winner: PartScore,
    /// Present when the pick has an exact score or an overtime win.
    pub score: Option<PartScore>,
}

/// Every scored part of a pick; stored as `pick_results.breakdown`. A part
/// the pick doesn't have is `None` (or absent from `maps`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakdown {
    pub winner: Option<PartScore>,
    pub series: Option<PartScore>,
    pub maps: Vec<MapScore>,
}

impl Breakdown {
    fn parts(&self) -> impl Iterator<Item = &PartScore> {
        self.winner.iter().chain(self.series.iter()).chain(
            self.maps
                .iter()
                .flat_map(|m| std::iter::once(&m.winner).chain(m.score.iter())),
        )
    }
}

/// The points of one pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scored {
    pub points: u32,
    pub state: ResultState,
    /// Parts still waiting for a result or data.
    pub pending_parts: u32,
    pub breakdown: Breakdown,
}

/// Scores `pick` against `result` with `rules`.
pub fn score(pick: &PickParts, result: &MatchResult, rules: &ScoringRules) -> Scored {
    let phase = result.phase;
    // What a part without its data becomes.
    let missing = if phase == Phase::Finished && result.deadline_passed {
        Outcome::Void
    } else {
        Outcome::Pending
    };
    let compare = |actual: Option<TeamId>, picked: TeamId| match actual {
        Some(a) if a == picked => Outcome::Hit,
        Some(_) => Outcome::Miss,
        None => missing,
    };

    let winner = pick.winner_team_id.map(|w| {
        let outcome = match phase {
            Phase::Void => Outcome::Void,
            Phase::Open | Phase::Live => Outcome::Pending,
            Phase::Finished => compare(result.match_winner(), w),
        };
        PartScore::new(outcome, rules.winner)
    });

    let series = pick.series.map(|s| {
        let outcome = match phase {
            Phase::Void => Outcome::Void,
            Phase::Open | Phase::Live => Outcome::Pending,
            Phase::Finished if !result.series_decided() => missing,
            Phase::Finished if result.series == s => Outcome::Hit,
            Phase::Finished => Outcome::Miss,
        };
        PartScore::new(outcome, rules.series)
    });

    let maps = pick
        .maps
        .iter()
        .map(|mp| {
            let has_score = mp.rounds.is_some() || mp.overtime;
            let (w, sc) = if phase == Phase::Void || result.map_not_played(mp.map_number) {
                (Outcome::Void, Outcome::Void)
            } else if phase == Phase::Open {
                (Outcome::Pending, Outcome::Pending)
            } else {
                let actual = result.maps.iter().find(|m| m.number == mp.map_number);
                match actual.filter(|m| m.status == MapStatus::Finished) {
                    Some(m) => {
                        let map_winner = m.winner.or_else(|| winner_from_rounds(result, m));
                        let w = compare(map_winner, mp.winner_team_id);
                        let sc = match m.rounds {
                            None => missing,
                            Some(rounds) => score_rounds(mp, map_winner, rounds),
                        };
                        (w, sc)
                    }
                    // Not finished yet, or no data for it in a finished match.
                    None if phase == Phase::Finished => (missing, missing),
                    None => (Outcome::Pending, Outcome::Pending),
                }
            };
            MapScore {
                map_number: mp.map_number,
                winner: PartScore::new(w, rules.map_winner),
                score: has_score.then(|| PartScore::new(sc, rules.map_score)),
            }
        })
        .collect();

    let breakdown = Breakdown {
        winner,
        series,
        maps,
    };
    let points = breakdown.parts().map(|p| p.points).sum();
    let pending_parts = breakdown
        .parts()
        .filter(|p| p.outcome == Outcome::Pending)
        .count();
    let state = match phase {
        Phase::Void => ResultState::Void,
        Phase::Finished => ResultState::Final,
        Phase::Open | Phase::Live => ResultState::Provisional,
    };
    Scored {
        points,
        state,
        pending_parts: u32::try_from(pending_parts).unwrap_or(u32::MAX),
        breakdown,
    }
}

/// The map winner by rounds, when the source gave rounds but no winner.
fn winner_from_rounds(result: &MatchResult, m: &MapResult) -> Option<TeamId> {
    let (a, b) = m.rounds?;
    match a.cmp(&b) {
        std::cmp::Ordering::Greater => result.team_a,
        std::cmp::Ordering::Less => result.team_b,
        std::cmp::Ordering::Equal => None,
    }
}

/// A map went to overtime: both teams reached 12 rounds (12:12 is played
/// out in overtime), so the final score is beyond regulation.
pub fn is_overtime(rounds: (u16, u16)) -> bool {
    rounds.0.min(rounds.1) > MAP_MAX_LOSER_ROUNDS && rounds.0.max(rounds.1) > MAP_WIN_ROUNDS
}

/// The map-score part against known rounds.
fn score_rounds(mp: &MapPick, map_winner: Option<TeamId>, rounds: (u16, u16)) -> Outcome {
    let hit = if mp.overtime {
        is_overtime(rounds) && map_winner == Some(mp.winner_team_id)
    } else {
        mp.rounds == Some(rounds)
    };
    if hit { Outcome::Hit } else { Outcome::Miss }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventId, MatchFields, MatchId};

    const A: TeamId = TeamId(1);
    const B: TeamId = TeamId(2);

    fn rules() -> ScoringRules {
        ScoringRules::default()
    }

    /// A finished match with the given maps (status finished, rounds or
    /// not); the series score and winner follow from the map winners.
    fn finished(format: MatchFormat, maps: &[(TeamId, Option<(u16, u16)>)]) -> MatchResult {
        let mut r = MatchResult {
            format,
            team_a: Some(A),
            team_b: Some(B),
            phase: Phase::Finished,
            winner: None,
            series: SeriesScore { a: 0, b: 0 },
            maps: Vec::new(),
            deadline_passed: false,
        };
        for (i, (w, rounds)) in maps.iter().enumerate() {
            if *w == A {
                r.series.a += 1;
            } else {
                r.series.b += 1;
            }
            r.maps.push(MapResult {
                number: u8::try_from(i + 1).unwrap(),
                status: MapStatus::Finished,
                winner: Some(*w),
                rounds: *rounds,
            });
        }
        for n in (maps.len() + 1)..=usize::from(format.max_maps()) {
            r.maps.push(MapResult {
                number: u8::try_from(n).unwrap(),
                status: MapStatus::NotPlayed,
                winner: None,
                rounds: None,
            });
        }
        r.winner = r.match_winner();
        r
    }

    fn map(n: u8, winner: TeamId) -> MapPick {
        MapPick {
            map_number: n,
            winner_team_id: winner,
            rounds: None,
            overtime: false,
        }
    }

    fn map_rounds(n: u8, winner: TeamId, rounds: (u16, u16)) -> MapPick {
        MapPick {
            rounds: Some(rounds),
            ..map(n, winner)
        }
    }

    fn map_ot(n: u8, winner: TeamId) -> MapPick {
        MapPick {
            overtime: true,
            ..map(n, winner)
        }
    }

    fn pick(winner: Option<TeamId>, series: Option<(u8, u8)>, maps: Vec<MapPick>) -> PickParts {
        PickParts {
            winner_team_id: winner,
            series: series.map(|(a, b)| SeriesScore { a, b }),
            maps,
        }
    }

    fn outcomes(s: &Scored) -> Vec<Outcome> {
        s.breakdown.parts().map(|p| p.outcome).collect()
    }

    use Outcome::{Hit, Miss, Pending, Void};

    // ---- rules --------------------------------------------------------------

    #[test]
    fn default_rules_and_max_points() {
        let r = rules();
        assert_eq!(
            (r.winner, r.series, r.map_winner, r.map_score),
            (3, 3, 1, 2)
        );
        let max: Vec<_> = MatchFormat::ALL.iter().map(|&f| r.max_points(f)).collect();
        assert_eq!(max, vec![6, 15, 21]);
    }

    #[test]
    fn rules_json_fills_defaults_and_rejects_unknown_fields() {
        let r: ScoringRules = serde_json::from_str(r#"{"winner": 5}"#).unwrap();
        assert_eq!(
            r,
            ScoringRules {
                winner: 5,
                ..ScoringRules::default()
            }
        );
        assert!(serde_json::from_str::<ScoringRules>(r#"{"winer": 5}"#).is_err());
        assert!(serde_json::from_str::<ScoringRules>(r#"{"winner": -1}"#).is_err());
        let too_much = ScoringRules {
            map_score: 101,
            ..ScoringRules::default()
        };
        assert!(too_much.validate().unwrap_err().0.contains("map_score"));
        assert!(rules().validate().is_ok());
    }

    #[test]
    fn perfect_picks_reach_the_maximum() {
        let r = rules();
        let bo1 = finished(MatchFormat::Bo1, &[(A, Some((13, 7)))]);
        let p = pick(Some(A), None, vec![map_rounds(1, A, (13, 7))]);
        assert_eq!(score(&p, &bo1, &r).points, 6);

        let bo3 = finished(
            MatchFormat::Bo3,
            &[(A, Some((13, 4))), (B, Some((14, 16))), (A, Some((13, 11)))],
        );
        let p = pick(
            Some(A),
            Some((2, 1)),
            vec![
                map_rounds(1, A, (13, 4)),
                map_ot(2, B),
                map_rounds(3, A, (13, 11)),
            ],
        );
        let s = score(&p, &bo3, &r);
        assert_eq!(s.points, 15);
        assert_eq!(s.state, ResultState::Final);
        assert_eq!(s.pending_parts, 0);

        let bo5 = finished(
            MatchFormat::Bo5,
            &[
                (A, Some((13, 1))),
                (B, Some((2, 13))),
                (A, Some((13, 3))),
                (B, Some((4, 13))),
                (A, Some((13, 5))),
            ],
        );
        let p = pick(
            Some(A),
            Some((3, 2)),
            vec![
                map_rounds(1, A, (13, 1)),
                map_rounds(2, B, (2, 13)),
                map_rounds(3, A, (13, 3)),
                map_rounds(4, B, (4, 13)),
                map_rounds(5, A, (13, 5)),
            ],
        );
        assert_eq!(score(&p, &bo5, &r).points, 21);
    }

    // ---- winner and series ------------------------------------------------------

    #[test]
    fn winner_and_series_hit_and_miss() {
        let r = rules();
        let res = finished(MatchFormat::Bo3, &[(A, None), (B, None), (A, None)]);
        type Case = (Option<TeamId>, Option<(u8, u8)>, u32, Vec<Outcome>);
        let cases: &[Case] = &[
            (Some(A), None, 3, vec![Hit]),
            (Some(B), None, 0, vec![Miss]),
            (Some(A), Some((2, 1)), 6, vec![Hit, Hit]),
            (Some(A), Some((2, 0)), 3, vec![Hit, Miss]),
            (Some(B), Some((1, 2)), 0, vec![Miss, Miss]),
        ];
        for (w, s, points, expect) in cases {
            let sc = score(&pick(*w, *s, vec![]), &res, &r);
            assert_eq!(sc.points, *points, "{w:?} {s:?}");
            assert_eq!(&outcomes(&sc), expect, "{w:?} {s:?}");
        }
    }

    #[test]
    fn winner_comes_from_the_series_when_not_stored() {
        let mut res = finished(MatchFormat::Bo3, &[(B, None), (B, None)]);
        res.winner = None;
        let sc = score(&pick(Some(B), Some((0, 2)), vec![]), &res, &rules());
        assert_eq!(sc.points, 6);
    }

    #[test]
    fn missing_series_data_is_pending_then_void() {
        let mut res = finished(MatchFormat::Bo3, &[]);
        res.maps.clear();
        let p = pick(Some(A), Some((2, 0)), vec![]);
        let sc = score(&p, &res, &rules());
        assert_eq!(outcomes(&sc), vec![Pending, Pending]);
        assert_eq!(sc.state, ResultState::Final);
        assert_eq!(sc.pending_parts, 2);
        res.deadline_passed = true;
        let sc = score(&p, &res, &rules());
        assert_eq!(outcomes(&sc), vec![Void, Void]);
        assert_eq!(sc.pending_parts, 0);
    }

    // ---- maps -------------------------------------------------------------------

    #[test]
    fn map_winner_and_regulation_score() {
        let r = rules();
        let res = finished(MatchFormat::Bo1, &[(A, Some((13, 11)))]);
        let cases: &[(MapPick, u32, Vec<Outcome>)] = &[
            (map(1, A), 1, vec![Hit]),
            (map(1, B), 0, vec![Miss]),
            (map_rounds(1, A, (13, 11)), 3, vec![Hit, Hit]),
            (map_rounds(1, A, (13, 10)), 1, vec![Hit, Miss]),
            (map_rounds(1, B, (11, 13)), 0, vec![Miss, Miss]),
            (map_ot(1, A), 1, vec![Hit, Miss]),
        ];
        for (mp, points, expect) in cases {
            let sc = score(&pick(None, None, vec![*mp]), &res, &r);
            assert_eq!(sc.points, *points, "{mp:?}");
            assert_eq!(&outcomes(&sc), expect, "{mp:?}");
        }
    }

    #[test]
    fn overtime_bucket() {
        let r = rules();
        let res = finished(MatchFormat::Bo1, &[(A, Some((16, 14)))]);
        let cases: &[(MapPick, u32, Vec<Outcome>)] = &[
            (map_ot(1, A), 3, vec![Hit, Hit]),
            (map_ot(1, B), 0, vec![Miss, Miss]),
            (map_rounds(1, A, (13, 11)), 1, vec![Hit, Miss]),
        ];
        for (mp, points, expect) in cases {
            let sc = score(&pick(None, None, vec![*mp]), &res, &r);
            assert_eq!(sc.points, *points, "{mp:?}");
            assert_eq!(&outcomes(&sc), expect, "{mp:?}");
        }
        assert!(is_overtime((16, 14)));
        assert!(is_overtime((19, 17)));
        assert!(is_overtime((13, 16)));
        assert!(!is_overtime((13, 11)));
        assert!(!is_overtime((13, 0)));
    }

    #[test]
    fn unplayed_maps_are_void() {
        let r = rules();
        // A 2:0 Bo3: map 3 is marked not_played.
        let res = finished(MatchFormat::Bo3, &[(A, None), (A, None)]);
        let p = pick(None, None, vec![map(1, A), map_rounds(3, B, (5, 13))]);
        let sc = score(&p, &res, &r);
        assert_eq!(outcomes(&sc), vec![Hit, Void, Void]);
        assert_eq!(sc.points, 1);

        // The same without map rows at all: map 3 is beyond the maps played.
        let mut bare = res.clone();
        bare.maps.retain(|m| m.number != 3);
        let sc = score(&pick(None, None, vec![map(3, A)]), &bare, &r);
        assert_eq!(outcomes(&sc), vec![Void]);
    }

    #[test]
    fn missing_round_scores_settle_the_winner_then_void() {
        let r = rules();
        // The free plan: map winners, no rounds.
        let mut res = finished(MatchFormat::Bo3, &[(A, None), (B, None), (A, None)]);
        let p = pick(
            None,
            None,
            vec![map_rounds(1, A, (13, 9)), map_ot(2, B), map(3, B)],
        );
        let sc = score(&p, &res, &r);
        assert_eq!(outcomes(&sc), vec![Hit, Pending, Hit, Pending, Miss]);
        assert_eq!(sc.points, 2);
        assert_eq!(sc.pending_parts, 2);
        assert_eq!(sc.state, ResultState::Final);

        res.deadline_passed = true;
        let sc = score(&p, &res, &r);
        assert_eq!(outcomes(&sc), vec![Hit, Void, Hit, Void, Miss]);
        assert_eq!(sc.points, 2);
        assert_eq!(sc.pending_parts, 0);

        // Late manual entry before the deadline settles the score part.
        res.deadline_passed = false;
        res.maps[0].rounds = Some((13, 9));
        let sc = score(&p, &res, &r);
        assert_eq!(sc.points, 4);
    }

    #[test]
    fn a_finished_map_without_a_winner_uses_rounds_or_waits() {
        let r = rules();
        let mut res = finished(MatchFormat::Bo1, &[(B, Some((7, 13)))]);
        res.maps[0].winner = None;
        let p = pick(None, None, vec![map_rounds(1, B, (7, 13))]);
        assert_eq!(score(&p, &res, &r).points, 3);

        res.maps[0].rounds = None;
        let sc = score(&p, &res, &r);
        assert_eq!(outcomes(&sc), vec![Pending, Pending]);
        res.deadline_passed = true;
        assert_eq!(outcomes(&score(&p, &res, &r)), vec![Void, Void]);
    }

    // ---- live ---------------------------------------------------------------------

    #[test]
    fn live_matches_score_finished_maps_provisionally() {
        let r = rules();
        let mut res = finished(MatchFormat::Bo3, &[(A, Some((13, 5)))]);
        res.phase = Phase::Live;
        res.winner = None;
        res.maps[1] = MapResult {
            number: 2,
            status: MapStatus::Live,
            winner: None,
            rounds: Some((4, 3)),
        };
        res.maps[2].status = MapStatus::Upcoming;
        let p = pick(
            Some(A),
            Some((2, 0)),
            vec![map_rounds(1, A, (13, 5)), map(2, A), map(3, B)],
        );
        let sc = score(&p, &res, &r);
        assert_eq!(
            outcomes(&sc),
            vec![Pending, Pending, Hit, Hit, Pending, Pending]
        );
        assert_eq!(sc.points, 3);
        assert_eq!(sc.state, ResultState::Provisional);
        assert_eq!(sc.pending_parts, 4);

        // The deadline never applies while live.
        res.deadline_passed = true;
        assert_eq!(score(&p, &res, &r).pending_parts, 4);
    }

    // ---- void -------------------------------------------------------------------

    fn stored(status: MatchStatus, voided: bool) -> Match {
        let mut f = MatchFields::scheduled(EventId(1), Some(A), Some(B), MatchFormat::Bo3, None);
        f.status = status;
        f.voided = voided;
        Match {
            id: MatchId(1),
            fields: f,
        }
    }

    #[test]
    fn canceled_forfeited_and_voided_matches_score_nothing() {
        let r = rules();
        let p = pick(Some(A), Some((2, 0)), vec![map(1, A), map_ot(2, A)]);
        // Canceled; a walkover is a canceled match with no winner (ADR 0001).
        let canceled = MatchResult::from_match(&stored(MatchStatus::Canceled, false), &[], false);
        // Voided by an admin after the match was played and decided.
        let mut done = stored(MatchStatus::Finished, true);
        done.fields.maps_won_a = 2;
        done.fields.winner_team_id = Some(A);
        let voided = MatchResult::from_match(&done, &[], false);
        for res in [canceled, voided] {
            assert_eq!(res.phase, Phase::Void);
            let sc = score(&p, &res, &r);
            assert_eq!(sc.points, 0);
            assert_eq!(sc.state, ResultState::Void);
            assert_eq!(sc.pending_parts, 0);
            assert!(outcomes(&sc).iter().all(|o| *o == Void));
        }
    }

    #[test]
    fn from_match_maps_status_to_phase() {
        let phase = |s, v| MatchResult::from_match(&stored(s, v), &[], false).phase;
        assert_eq!(phase(MatchStatus::Scheduled, false), Phase::Open);
        assert_eq!(phase(MatchStatus::Postponed, false), Phase::Open);
        assert_eq!(phase(MatchStatus::Live, false), Phase::Live);
        assert_eq!(phase(MatchStatus::Finished, false), Phase::Finished);
        assert_eq!(phase(MatchStatus::Canceled, false), Phase::Void);
        assert_eq!(phase(MatchStatus::Scheduled, true), Phase::Void);
    }

    #[test]
    fn breakdown_serializes_with_snake_case_outcomes() {
        let res = finished(MatchFormat::Bo1, &[(A, None)]);
        let sc = score(&pick(Some(A), None, vec![map_ot(1, A)]), &res, &rules());
        let json = serde_json::to_value(&sc.breakdown).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "winner": { "outcome": "hit", "points": 3 },
                "series": null,
                "maps": [{
                    "map_number": 1,
                    "winner": { "outcome": "hit", "points": 1 },
                    "score": { "outcome": "pending", "points": 0 },
                }],
            })
        );
        let back: Breakdown = serde_json::from_value(json).unwrap();
        assert_eq!(back, sc.breakdown);
    }
}
