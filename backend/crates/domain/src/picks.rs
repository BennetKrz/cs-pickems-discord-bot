//! Pick rules (T10): which picks are valid, when a match can be picked,
//! and the aggregates everyone sees.
//!
//! A pick has three optional parts: the winner, the series score (Bo3/Bo5
//! only) and map picks keyed by map number. All scores are relative to the
//! match's team A/B. See `docs/tickets/T10-pick-rules-api.md` and
//! `docs/adr/0010-pick-rules-and-api.md`.

use std::collections::HashSet;
use std::fmt;

use chrono::{DateTime, Utc};

use crate::model::{
    MapPick, MatchFields, MatchFormat, MatchStatus, PickParts, SeriesScore, TeamId,
};

/// Rounds a team needs to win a map in regulation.
pub const MAP_WIN_ROUNDS: u16 = 13;
/// Highest loser score of a regulation map; 12:12 goes to overtime.
pub const MAP_MAX_LOSER_ROUNDS: u16 = 11;

/// The series scores that can be picked for a format, team A's wins first:
/// none for Bo1 (there the winner says it all), `2:0 2:1 1:2 0:2` for Bo3,
/// `3:0 3:1 3:2 2:3 1:3 0:3` for Bo5.
pub fn allowed_series(format: MatchFormat) -> Vec<SeriesScore> {
    if format == MatchFormat::Bo1 {
        return Vec::new();
    }
    let w = format.wins_needed();
    let a_wins = (0..w).map(|l| SeriesScore { a: w, b: l });
    let b_wins = (0..w).rev().map(|l| SeriesScore { a: l, b: w });
    a_wins.chain(b_wins).collect()
}

/// Why a pick is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickError {
    /// No part is set. The API treats saving an empty pick as a delete.
    Empty,
    /// The picked winner is not a team of the match.
    WinnerNotInMatch,
    /// The series score is not allowed for the format (any score in Bo1).
    SeriesNotAllowed {
        format: MatchFormat,
        score: SeriesScore,
    },
    /// The series score names the other team as winner.
    SeriesContradictsWinner,
    MapNumberOutOfRange {
        map_number: u8,
        max: u8,
    },
    DuplicateMap {
        map_number: u8,
    },
    MapWinnerNotInMatch {
        map_number: u8,
    },
    /// Not 13:0–13:11 with the 13 on the winner's side.
    MapScoreInvalid {
        map_number: u8,
        rounds: (u16, u16),
    },
    /// Both an exact score and an overtime win.
    MapScoreAndOvertime {
        map_number: u8,
    },
}

impl fmt::Display for PickError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PickError::Empty => f.write_str("the pick has no parts"),
            PickError::WinnerNotInMatch => f.write_str("the winner is not a team of this match"),
            PickError::SeriesNotAllowed { format, score } => {
                if *format == MatchFormat::Bo1 {
                    write!(f, "a Bo1 has no series score; pick the winner instead")
                } else {
                    write!(f, "{}:{} is not a {format} series score", score.a, score.b)
                }
            }
            PickError::SeriesContradictsWinner => {
                f.write_str("the series score names the other team as winner")
            }
            PickError::MapNumberOutOfRange { map_number, max } => {
                write!(
                    f,
                    "map {map_number} does not exist; this match has maps 1 to {max}"
                )
            }
            PickError::DuplicateMap { map_number } => {
                write!(f, "map {map_number} is picked more than once")
            }
            PickError::MapWinnerNotInMatch { map_number } => {
                write!(
                    f,
                    "the winner of map {map_number} is not a team of this match"
                )
            }
            PickError::MapScoreInvalid { map_number, rounds } => write!(
                f,
                "{}:{} is not a valid score for map {map_number}: the winner has 13 rounds, \
                 the loser 0 to 11; pick an overtime win for anything longer",
                rounds.0, rounds.1
            ),
            PickError::MapScoreAndOvertime { map_number } => write!(
                f,
                "map {map_number} has both an exact score and an overtime win"
            ),
        }
    }
}

impl std::error::Error for PickError {}

/// Checks `parts` against the rules for a match of `format` between
/// `team_a` and `team_b`, and returns them normalized: a series score
/// without a winner sets the winner, and map picks are sorted by number.
pub fn validate(
    format: MatchFormat,
    team_a: TeamId,
    team_b: TeamId,
    parts: &PickParts,
) -> Result<PickParts, PickError> {
    if parts.winner_team_id.is_none() && parts.series.is_none() && parts.maps.is_empty() {
        return Err(PickError::Empty);
    }
    let in_match = |t: TeamId| t == team_a || t == team_b;

    if let Some(w) = parts.winner_team_id
        && !in_match(w)
    {
        return Err(PickError::WinnerNotInMatch);
    }

    let mut winner = parts.winner_team_id;
    if let Some(score) = parts.series {
        if !allowed_series(format).contains(&score) {
            return Err(PickError::SeriesNotAllowed { format, score });
        }
        let series_winner = if score.a > score.b { team_a } else { team_b };
        match winner {
            Some(w) if w != series_winner => return Err(PickError::SeriesContradictsWinner),
            _ => winner = Some(series_winner),
        }
    }

    let max = format.max_maps();
    let mut seen = HashSet::new();
    for m in &parts.maps {
        if m.map_number == 0 || m.map_number > max {
            return Err(PickError::MapNumberOutOfRange {
                map_number: m.map_number,
                max,
            });
        }
        if !seen.insert(m.map_number) {
            return Err(PickError::DuplicateMap {
                map_number: m.map_number,
            });
        }
        if !in_match(m.winner_team_id) {
            return Err(PickError::MapWinnerNotInMatch {
                map_number: m.map_number,
            });
        }
        if let Some(rounds) = m.rounds {
            if m.overtime {
                return Err(PickError::MapScoreAndOvertime {
                    map_number: m.map_number,
                });
            }
            let (win, lose) = if m.winner_team_id == team_a {
                rounds
            } else {
                (rounds.1, rounds.0)
            };
            if win != MAP_WIN_ROUNDS || lose > MAP_MAX_LOSER_ROUNDS {
                return Err(PickError::MapScoreInvalid {
                    map_number: m.map_number,
                    rounds,
                });
            }
        }
    }

    let mut maps: Vec<MapPick> = parts.maps.clone();
    maps.sort_by_key(|m| m.map_number);
    Ok(PickParts {
        winner_team_id: winner,
        series: parts.series,
        maps,
    })
}

/// Why a match can't be picked right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unpickable {
    /// The match is locked: the lock ticker ran, `lock_at` has passed, or
    /// it is live or finished. Only an admin reopen unlocks it.
    Locked,
    NotPickable(NotPickableReason),
}

/// Why an unlocked match is still not open for picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotPickableReason {
    /// The guild does not follow the match's event.
    NotFollowed,
    /// A team is still TBD.
    TeamsUnknown,
    /// Canceled or postponed.
    Status(MatchStatus),
    /// No start time yet, so no lock time.
    NoStartTime,
    /// An admin voided the match (T05).
    Voided,
}

impl fmt::Display for Unpickable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unpickable::Locked => f.write_str("picks for this match are locked"),
            Unpickable::NotPickable(r) => match r {
                NotPickableReason::NotFollowed => {
                    f.write_str("this match's event is not followed in this server")
                }
                NotPickableReason::TeamsUnknown => f.write_str("the teams are not known yet"),
                NotPickableReason::Status(s) => write!(f, "the match is {s}"),
                NotPickableReason::NoStartTime => f.write_str("the match has no start time yet"),
                NotPickableReason::Voided => f.write_str("the match was voided"),
            },
        }
    }
}

/// Whether the picks of a match are locked at `now`. The server clock
/// decides: a match whose `lock_at` has passed is locked even before the
/// lock ticker marks it (same boundary as the ticker: `lock_at <= now`).
pub fn is_locked(m: &MatchFields, now: DateTime<Utc>) -> bool {
    m.locked
        || m.lock_at.is_some_and(|at| at <= now)
        || matches!(m.status, MatchStatus::Live | MatchStatus::Finished)
}

/// Whether a member can create, edit or delete a pick for `m` at `now`.
/// `followed` says whether the guild follows the match's event. The lock
/// is checked first, so a late request always gets [`Unpickable::Locked`].
pub fn check_pickable(
    m: &MatchFields,
    followed: bool,
    now: DateTime<Utc>,
) -> Result<(), Unpickable> {
    use NotPickableReason as R;
    if is_locked(m, now) {
        return Err(Unpickable::Locked);
    }
    let reason = if !followed {
        R::NotFollowed
    } else if m.team_a_id.is_none() || m.team_b_id.is_none() {
        R::TeamsUnknown
    } else if m.status != MatchStatus::Scheduled {
        R::Status(m.status)
    } else if m.lock_at.is_none() {
        R::NoStartTime
    } else if m.voided {
        R::Voided
    } else {
        return Ok(());
    };
    Err(Unpickable::NotPickable(reason))
}

// ---- completeness (T12) --------------------------------------------------------

/// How much of a pick is filled in, for the "my status" chip of the match
/// list (T12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    /// Some parts are set, but not every part [`completeness`] asks for.
    Partial,
    /// Every part is set; map scores stay optional.
    Full,
}

/// Whether a (validated) pick is full: a winner, a series score (Bo3/Bo5),
/// and a map winner for every map the series implies (2:1 → maps 1–3,
/// 2:0 → maps 1–2; Bo1 → map 1). Map scores are optional. Decided for T12,
/// see `docs/adr/0012-matches-views.md`.
pub fn completeness(format: MatchFormat, parts: &PickParts) -> Completeness {
    let maps_needed = match (format, parts.series) {
        (MatchFormat::Bo1, _) => Some(1),
        (_, Some(s)) => Some(s.a + s.b),
        (_, None) => None,
    };
    let full = parts.winner_team_id.is_some()
        && maps_needed
            .is_some_and(|n| (1..=n).all(|i| parts.maps.iter().any(|m| m.map_number == i)));
    if full {
        Completeness::Full
    } else {
        Completeness::Partial
    }
}

// ---- aggregates --------------------------------------------------------------

/// How the picks that have some part split between team A and team B.
/// Percentages are whole numbers that add up to 100, or both 0 if no pick
/// has the part.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Split {
    pub a: u32,
    pub b: u32,
    pub pct_a: u8,
    pub pct_b: u8,
}

impl Split {
    fn new(a: u32, b: u32) -> Self {
        let pct = percentages(&[a, b]);
        Self {
            a,
            b,
            pct_a: pct[0],
            pct_b: pct[1],
        }
    }
}

/// How many picks chose one series score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SeriesBucket {
    pub a: u8,
    pub b: u8,
    pub count: u32,
    pub pct: u8,
}

/// The winner split of one map number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MapSplit {
    pub map_number: u8,
    #[serde(flatten)]
    pub split: Split,
}

/// The aggregates of all picks of a match.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PickStats {
    /// Number of picks.
    pub total: u32,
    pub winner: Split,
    /// Every allowed score of the format, in [`allowed_series`] order,
    /// including those nobody picked. Empty for Bo1.
    pub series: Vec<SeriesBucket>,
    /// One entry per map number 1…N of the format.
    pub maps: Vec<MapSplit>,
}

/// Aggregates the (validated) picks of a match between `team_a` and
/// `team_b`.
pub fn aggregate<'a>(
    format: MatchFormat,
    team_a: Option<TeamId>,
    team_b: Option<TeamId>,
    picks: impl IntoIterator<Item = &'a PickParts>,
) -> PickStats {
    let series_keys = allowed_series(format);
    let mut series_counts = vec![0u32; series_keys.len()];
    let max = usize::from(format.max_maps());
    let mut map_counts = vec![(0u32, 0u32); max];
    let (mut win_a, mut win_b, mut total) = (0u32, 0u32, 0u32);

    let count = |t: TeamId, a: &mut u32, b: &mut u32| {
        if Some(t) == team_a {
            *a += 1;
        } else if Some(t) == team_b {
            *b += 1;
        }
    };

    for p in picks {
        total += 1;
        if let Some(w) = p.winner_team_id {
            count(w, &mut win_a, &mut win_b);
        }
        if let Some(s) = p.series
            && let Some(i) = series_keys.iter().position(|k| *k == s)
        {
            series_counts[i] += 1;
        }
        for m in &p.maps {
            if let Some((a, b)) = usize::from(m.map_number)
                .checked_sub(1)
                .and_then(|i| map_counts.get_mut(i))
            {
                count(m.winner_team_id, a, b);
            }
        }
    }

    let series_pct = percentages(&series_counts);
    PickStats {
        total,
        winner: Split::new(win_a, win_b),
        series: series_keys
            .iter()
            .zip(series_counts.iter().zip(series_pct))
            .map(|(k, (&count, pct))| SeriesBucket {
                a: k.a,
                b: k.b,
                count,
                pct,
            })
            .collect(),
        maps: map_counts
            .into_iter()
            .zip(1u8..)
            .map(|((a, b), map_number)| MapSplit {
                map_number,
                split: Split::new(a, b),
            })
            .collect(),
    }
}

/// Whole percentages of `counts` that add up to 100 (largest remainder
/// method; ties go to the earlier entry). All 0 if the counts sum to 0.
fn percentages(counts: &[u32]) -> Vec<u8> {
    let total: u64 = counts.iter().map(|&c| u64::from(c)).sum();
    if total == 0 {
        return vec![0; counts.len()];
    }
    let mut pct: Vec<u64> = counts.iter().map(|&c| u64::from(c) * 100 / total).collect();
    let mut order: Vec<usize> = (0..counts.len()).collect();
    // Stable sort keeps index order among equal remainders.
    order.sort_by_key(|&i| std::cmp::Reverse(u64::from(counts[i]) * 100 % total));
    let missing = 100 - pct.iter().sum::<u64>();
    for &i in order.iter().take(usize::try_from(missing).unwrap_or(0)) {
        pct[i] += 1;
    }
    pct.into_iter()
        .map(|p| u8::try_from(p).unwrap_or(100))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventId, MatchFormat::*};
    use chrono::TimeZone;

    const A: TeamId = TeamId(10);
    const B: TeamId = TeamId(20);
    const C: TeamId = TeamId(30);

    fn s(a: u8, b: u8) -> SeriesScore {
        SeriesScore { a, b }
    }

    fn map(n: u8, w: TeamId) -> MapPick {
        MapPick {
            map_number: n,
            winner_team_id: w,
            rounds: None,
            overtime: false,
        }
    }

    fn map_score(n: u8, w: TeamId, a: u16, b: u16) -> MapPick {
        MapPick {
            rounds: Some((a, b)),
            ..map(n, w)
        }
    }

    fn map_ot(n: u8, w: TeamId) -> MapPick {
        MapPick {
            overtime: true,
            ..map(n, w)
        }
    }

    fn parts(winner: Option<TeamId>, series: Option<SeriesScore>, maps: Vec<MapPick>) -> PickParts {
        PickParts {
            winner_team_id: winner,
            series,
            maps,
        }
    }

    fn check(format: MatchFormat, p: &PickParts) -> Result<PickParts, PickError> {
        validate(format, A, B, p)
    }

    #[test]
    fn allowed_series_per_format() {
        assert!(allowed_series(Bo1).is_empty());
        assert_eq!(
            allowed_series(Bo3),
            vec![s(2, 0), s(2, 1), s(1, 2), s(0, 2)]
        );
        assert_eq!(
            allowed_series(Bo5),
            vec![s(3, 0), s(3, 1), s(3, 2), s(2, 3), s(1, 3), s(0, 3)]
        );
    }

    #[test]
    fn series_scores_table() {
        // (format, score, accepted)
        let mut cases = Vec::new();
        for f in [Bo1, Bo3, Bo5] {
            for a in 0..=4u8 {
                for b in 0..=4u8 {
                    cases.push((f, s(a, b), allowed_series(f).contains(&s(a, b))));
                }
            }
        }
        // Spot checks that the table above means what the ticket says.
        for (f, sc, ok) in [
            (Bo1, s(1, 0), false),
            (Bo1, s(0, 1), false),
            (Bo3, s(2, 0), true),
            (Bo3, s(2, 1), true),
            (Bo3, s(1, 2), true),
            (Bo3, s(0, 2), true),
            (Bo3, s(2, 2), false),
            (Bo3, s(3, 0), false),
            (Bo3, s(1, 0), false),
            (Bo3, s(0, 0), false),
            (Bo5, s(3, 0), true),
            (Bo5, s(3, 2), true),
            (Bo5, s(2, 3), true),
            (Bo5, s(0, 3), true),
            (Bo5, s(2, 1), false),
            (Bo5, s(3, 3), false),
            (Bo5, s(4, 0), false),
        ] {
            assert!(cases.contains(&(f, sc, ok)), "{f} {sc:?}");
        }
        for (f, sc, ok) in cases {
            let res = check(f, &parts(None, Some(sc), vec![]));
            assert_eq!(res.is_ok(), ok, "{f} {}:{} → {res:?}", sc.a, sc.b);
            if !ok {
                assert_eq!(
                    res,
                    Err(PickError::SeriesNotAllowed {
                        format: f,
                        score: sc
                    })
                );
            }
        }
    }

    #[test]
    fn series_sets_the_winner_and_must_agree_with_it() {
        let got = check(Bo3, &parts(None, Some(s(1, 2)), vec![])).unwrap();
        assert_eq!(got.winner_team_id, Some(B));
        let got = check(Bo5, &parts(Some(A), Some(s(3, 1)), vec![])).unwrap();
        assert_eq!(got.winner_team_id, Some(A));
        assert_eq!(
            check(Bo3, &parts(Some(A), Some(s(0, 2)), vec![])),
            Err(PickError::SeriesContradictsWinner)
        );
    }

    #[test]
    fn winner_must_be_a_team_of_the_match() {
        assert_eq!(
            check(Bo1, &parts(Some(C), None, vec![])),
            Err(PickError::WinnerNotInMatch)
        );
    }

    #[test]
    fn map_numbers_table() {
        for f in [Bo1, Bo3, Bo5] {
            let max = f.max_maps();
            for n in 0..=6u8 {
                let res = check(f, &parts(None, None, vec![map(n, A)]));
                if (1..=max).contains(&n) {
                    assert!(res.is_ok(), "{f} map {n}: {res:?}");
                } else {
                    assert_eq!(
                        res,
                        Err(PickError::MapNumberOutOfRange { map_number: n, max }),
                        "{f} map {n}"
                    );
                }
            }
        }
        assert_eq!(
            check(Bo3, &parts(None, None, vec![map(2, A), map(2, B)])),
            Err(PickError::DuplicateMap { map_number: 2 })
        );
    }

    #[test]
    fn map_scores_table() {
        // (map pick, expected error); every case is valid in every format.
        let cases: Vec<(MapPick, Option<PickError>)> = vec![
            (map(1, A), None),
            (map_ot(1, A), None),
            (map_ot(1, B), None),
            (map_score(1, A, 13, 0), None),
            (map_score(1, A, 13, 11), None),
            (map_score(1, B, 0, 13), None),
            (map_score(1, B, 11, 13), None),
            (map_score(1, B, 7, 13), None),
            (
                map_score(1, A, 13, 12),
                Some(PickError::MapScoreInvalid {
                    map_number: 1,
                    rounds: (13, 12),
                }),
            ),
            (
                map_score(1, A, 12, 10),
                Some(PickError::MapScoreInvalid {
                    map_number: 1,
                    rounds: (12, 10),
                }),
            ),
            (
                map_score(1, A, 14, 12),
                Some(PickError::MapScoreInvalid {
                    map_number: 1,
                    rounds: (14, 12),
                }),
            ),
            (
                map_score(1, A, 16, 14),
                Some(PickError::MapScoreInvalid {
                    map_number: 1,
                    rounds: (16, 14),
                }),
            ),
            (
                map_score(1, A, 13, 13),
                Some(PickError::MapScoreInvalid {
                    map_number: 1,
                    rounds: (13, 13),
                }),
            ),
            // Score orientation is A:B, so the winner's 13 must be on its side.
            (
                map_score(1, A, 5, 13),
                Some(PickError::MapScoreInvalid {
                    map_number: 1,
                    rounds: (5, 13),
                }),
            ),
            (
                map_score(1, B, 13, 5),
                Some(PickError::MapScoreInvalid {
                    map_number: 1,
                    rounds: (13, 5),
                }),
            ),
            (
                MapPick {
                    overtime: true,
                    ..map_score(1, A, 13, 5)
                },
                Some(PickError::MapScoreAndOvertime { map_number: 1 }),
            ),
            (
                map(1, C),
                Some(PickError::MapWinnerNotInMatch { map_number: 1 }),
            ),
        ];
        for f in [Bo1, Bo3, Bo5] {
            for (m, err) in &cases {
                let res = check(f, &parts(None, None, vec![*m]));
                assert_eq!(res.err(), err.clone(), "{f} {m:?}");
            }
        }
    }

    #[test]
    fn empty_pick_is_its_own_error() {
        for f in [Bo1, Bo3, Bo5] {
            assert_eq!(check(f, &PickParts::default()), Err(PickError::Empty));
        }
    }

    #[test]
    fn winner_only_map_only_and_full_picks_are_accepted() {
        for f in [Bo1, Bo3, Bo5] {
            assert!(
                check(f, &parts(Some(A), None, vec![])).is_ok(),
                "{f} winner only"
            );
            assert!(
                check(f, &parts(None, None, vec![map(1, B)])).is_ok(),
                "{f} map only"
            );
        }
        let full = parts(
            Some(A),
            Some(s(2, 1)),
            vec![map_ot(3, A), map_score(1, A, 13, 9), map(2, B)],
        );
        let got = check(Bo3, &full).unwrap();
        let numbers: Vec<u8> = got.maps.iter().map(|m| m.map_number).collect();
        assert_eq!(numbers, vec![1, 2, 3], "maps sorted by number");
        // Map picks and series pick need not agree.
        let disagree = parts(None, Some(s(2, 0)), vec![map(1, B), map(2, B)]);
        assert!(check(Bo3, &disagree).is_ok());
    }

    // ---- pickability ---------------------------------------------------------

    fn at(h: u32, m: u32, sec: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, h, m, sec).unwrap()
    }

    fn open_match() -> MatchFields {
        MatchFields::scheduled(EventId(1), Some(A), Some(B), Bo3, Some(at(18, 0, 0)))
    }

    #[test]
    fn pickability_table() {
        use NotPickableReason as R;
        let now = at(12, 0, 0);
        let tweak = |f: fn(&mut MatchFields)| {
            let mut m = open_match();
            f(&mut m);
            m
        };
        /// (name, match, followed, now, expected)
        type Case = (
            &'static str,
            MatchFields,
            bool,
            DateTime<Utc>,
            Result<(), Unpickable>,
        );
        let cases: Vec<Case> = vec![
            ("open", open_match(), true, now, Ok(())),
            (
                "1 s before lock",
                open_match(),
                true,
                at(17, 59, 59),
                Ok(()),
            ),
            (
                "at lock_at",
                open_match(),
                true,
                at(18, 0, 0),
                Err(Unpickable::Locked),
            ),
            (
                "1 s after lock_at, ticker not run",
                open_match(),
                true,
                at(18, 0, 1),
                Err(Unpickable::Locked),
            ),
            (
                "locked flag",
                tweak(|m| m.locked = true),
                true,
                now,
                Err(Unpickable::Locked),
            ),
            (
                "live",
                tweak(|m| m.status = MatchStatus::Live),
                true,
                now,
                Err(Unpickable::Locked),
            ),
            (
                "finished",
                tweak(|m| m.status = MatchStatus::Finished),
                true,
                now,
                Err(Unpickable::Locked),
            ),
            (
                "locked beats unfollowed",
                tweak(|m| m.locked = true),
                false,
                now,
                Err(Unpickable::Locked),
            ),
            (
                "unfollowed",
                open_match(),
                false,
                now,
                Err(Unpickable::NotPickable(R::NotFollowed)),
            ),
            (
                "team A TBD",
                tweak(|m| m.team_a_id = None),
                true,
                now,
                Err(Unpickable::NotPickable(R::TeamsUnknown)),
            ),
            (
                "team B TBD",
                tweak(|m| m.team_b_id = None),
                true,
                now,
                Err(Unpickable::NotPickable(R::TeamsUnknown)),
            ),
            (
                "canceled",
                tweak(|m| m.status = MatchStatus::Canceled),
                true,
                now,
                Err(Unpickable::NotPickable(R::Status(MatchStatus::Canceled))),
            ),
            (
                "postponed",
                tweak(|m| m.status = MatchStatus::Postponed),
                true,
                now,
                Err(Unpickable::NotPickable(R::Status(MatchStatus::Postponed))),
            ),
            (
                "no start time",
                tweak(|m| m.lock_at = None),
                true,
                now,
                Err(Unpickable::NotPickable(R::NoStartTime)),
            ),
            (
                "voided",
                tweak(|m| m.voided = true),
                true,
                now,
                Err(Unpickable::NotPickable(R::Voided)),
            ),
        ];
        for (name, m, followed, now, want) in cases {
            assert_eq!(check_pickable(&m, followed, now), want, "{name}");
        }
    }

    #[test]
    fn rescheduled_lock_moves_with_lock_at() {
        let mut m = open_match();
        m.lock_at = Some(at(20, 0, 0));
        assert_eq!(check_pickable(&m, true, at(19, 0, 0)), Ok(()));
    }

    // ---- aggregates ----------------------------------------------------------

    #[test]
    fn percentages_add_up_to_100() {
        assert_eq!(percentages(&[0, 0]), vec![0, 0]);
        assert_eq!(percentages(&[1, 0]), vec![100, 0]);
        assert_eq!(percentages(&[1, 1, 1]), vec![34, 33, 33]);
        assert_eq!(percentages(&[2, 1]), vec![67, 33]);
        assert_eq!(percentages(&[1, 2, 0, 4]), vec![14, 29, 0, 57]);
        for counts in [[3u32, 3, 1], [7, 0, 2], [1, 1, 5], [0, 9, 9]] {
            assert_eq!(
                percentages(&counts)
                    .iter()
                    .map(|&p| u32::from(p))
                    .sum::<u32>(),
                100
            );
        }
    }

    #[test]
    fn aggregate_of_no_picks() {
        let st = aggregate(Bo3, Some(A), Some(B), &[]);
        assert_eq!(st.total, 0);
        assert_eq!(st.winner, Split::default());
        assert_eq!(st.series.len(), 4);
        assert!(st.series.iter().all(|b| b.count == 0 && b.pct == 0));
        assert_eq!(st.maps.len(), 3);
    }

    #[test]
    fn aggregate_counts_each_part() {
        let picks = [
            parts(Some(A), Some(s(2, 0)), vec![map(1, A), map(2, A)]),
            parts(Some(A), Some(s(2, 1)), vec![map(1, B)]),
            parts(Some(B), None, vec![]),
            parts(None, None, vec![map(3, B)]),
        ];
        let st = aggregate(Bo3, Some(A), Some(B), &picks);
        assert_eq!(st.total, 4);
        assert_eq!(
            st.winner,
            Split {
                a: 2,
                b: 1,
                pct_a: 67,
                pct_b: 33
            }
        );
        let series: Vec<(u8, u8, u32, u8)> = st
            .series
            .iter()
            .map(|b| (b.a, b.b, b.count, b.pct))
            .collect();
        assert_eq!(
            series,
            vec![(2, 0, 1, 50), (2, 1, 1, 50), (1, 2, 0, 0), (0, 2, 0, 0)]
        );
        let maps: Vec<(u8, Split)> = st.maps.iter().map(|m| (m.map_number, m.split)).collect();
        assert_eq!(
            maps,
            vec![
                (
                    1,
                    Split {
                        a: 1,
                        b: 1,
                        pct_a: 50,
                        pct_b: 50
                    }
                ),
                (
                    2,
                    Split {
                        a: 1,
                        b: 0,
                        pct_a: 100,
                        pct_b: 0
                    }
                ),
                (
                    3,
                    Split {
                        a: 0,
                        b: 1,
                        pct_a: 0,
                        pct_b: 100
                    }
                ),
            ]
        );
    }

    #[test]
    fn aggregate_bo1_has_no_series_and_one_map() {
        let picks = [parts(Some(B), None, vec![map_ot(1, B)])];
        let st = aggregate(Bo1, Some(A), Some(B), &picks);
        assert!(st.series.is_empty());
        assert_eq!(st.maps.len(), 1);
        assert_eq!(st.winner.pct_b, 100);
    }

    #[test]
    fn stats_serialize_flat() {
        let st = aggregate(
            Bo1,
            Some(A),
            Some(B),
            &[parts(Some(A), None, vec![map(1, A)])],
        );
        let v = serde_json::to_value(&st).unwrap();
        assert_eq!(v["winner"]["pct_a"], 100);
        assert_eq!(v["maps"][0]["map_number"], 1);
        assert_eq!(v["maps"][0]["a"], 1);
    }

    #[test]
    fn completeness_follows_the_series_pick() {
        use Completeness::{Full, Partial};
        let cases: Vec<(MatchFormat, PickParts, Completeness)> = vec![
            // Bo1: winner plus map 1, score optional.
            (Bo1, parts(Some(A), None, vec![]), Partial),
            (Bo1, parts(None, None, vec![map(1, A)]), Partial),
            (Bo1, parts(Some(A), None, vec![map(1, B)]), Full),
            (Bo1, parts(Some(A), None, vec![map_ot(1, A)]), Full),
            // Bo3: the series decides how many maps are needed.
            (
                Bo3,
                parts(Some(A), None, vec![map(1, A), map(2, A)]),
                Partial,
            ),
            (Bo3, parts(Some(A), Some(s(2, 0)), vec![]), Partial),
            (Bo3, parts(Some(A), Some(s(2, 0)), vec![map(1, A)]), Partial),
            (
                Bo3,
                parts(Some(A), Some(s(2, 0)), vec![map(1, A), map(2, A)]),
                Full,
            ),
            (
                Bo3,
                parts(Some(A), Some(s(2, 0)), vec![map(1, A), map(3, A)]),
                Partial,
            ),
            (
                Bo3,
                parts(Some(B), Some(s(1, 2)), vec![map(1, A), map(2, B)]),
                Partial,
            ),
            (
                Bo3,
                parts(
                    Some(B),
                    Some(s(1, 2)),
                    vec![map_score(1, A, 13, 4), map(2, B), map_ot(3, B)],
                ),
                Full,
            ),
            // Bo5: 3:2 needs all five maps.
            (
                Bo5,
                parts(
                    Some(A),
                    Some(s(3, 2)),
                    vec![map(1, A), map(2, B), map(3, A), map(4, B)],
                ),
                Partial,
            ),
            (
                Bo5,
                parts(
                    Some(A),
                    Some(s(3, 2)),
                    vec![map(1, A), map(2, B), map(3, A), map(4, B), map(5, A)],
                ),
                Full,
            ),
            (
                Bo5,
                parts(
                    Some(A),
                    Some(s(3, 0)),
                    vec![map(1, A), map(2, A), map(3, A)],
                ),
                Full,
            ),
        ];
        for (format, p, want) in cases {
            assert_eq!(completeness(format, &p), want, "{format} {p:?}");
        }
    }

    #[test]
    fn completeness_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(Completeness::Partial).unwrap(),
            "partial"
        );
        assert_eq!(serde_json::to_value(Completeness::Full).unwrap(), "full");
    }
}
