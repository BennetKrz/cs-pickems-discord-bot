//! The server leaderboard (T14): per-member totals from settled picks, the
//! tie-break, shared ranks and the rank change since the previous match
//! day. Pure; `store::leaderboard` feeds it from `pick_results`. See
//! `docs/adr/0016-leaderboard.md`.
//!
//! - **Who appears:** members with at least one non-void result
//!   (provisional or final).
//! - **Points** are the sum of their `pick_results.points`, provisional
//!   ones included; `provisional_points` is the part from live matches.
//! - **Tie-break:** points, then exact hits (series plus map scores), then
//!   winner accuracy. Still equal means a shared rank (1, 1, 3).
//! - **Match day:** a calendar day in the guild's time zone, dated by match
//!   start. The previous rank is the rank without the matches of the
//!   latest match day, computed from the same (current) results.

use std::cmp::Ordering;
use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;

use crate::localtime;
use crate::model::ResultState;
use crate::scoring::{Breakdown, Outcome, PartScore};

/// One settled pick, as the leaderboard needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredPick {
    pub user_id: String,
    pub points: i64,
    pub state: ResultState,
    pub breakdown: Breakdown,
    /// When the match started (else when it was scheduled); dates the pick
    /// to a match day.
    pub match_at: Option<DateTime<Utc>>,
}

/// A member's totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    pub user_id: String,
    /// Sum of all results, provisional included.
    pub points: i64,
    /// The part of `points` from provisional (live) results.
    pub provisional_points: i64,
    /// Results of live matches: their points can still move.
    pub provisional_picks: u32,
    /// Results that are not void.
    pub settled_picks: u32,
    pub winner_hits: u32,
    /// Winner parts that are hit or miss (not pending, not void).
    pub winner_decided: u32,
    /// Exact series scores.
    pub series_hits: u32,
    /// Series parts that are hit or miss.
    pub series_decided: u32,
    /// Correct map winners.
    pub map_winner_hits: u32,
    /// Map winner parts that are hit or miss.
    pub map_winner_decided: u32,
    /// Exact map scores (or correct overtime winners).
    pub map_hits: u32,
    /// Map score parts that are hit or miss.
    pub map_score_decided: u32,
}

impl Standing {
    /// A member without any result.
    pub fn new(user_id: &str) -> Self {
        Self {
            user_id: user_id.to_owned(),
            points: 0,
            provisional_points: 0,
            provisional_picks: 0,
            settled_picks: 0,
            winner_hits: 0,
            winner_decided: 0,
            series_hits: 0,
            series_decided: 0,
            map_winner_hits: 0,
            map_winner_decided: 0,
            map_hits: 0,
            map_score_decided: 0,
        }
    }

    /// Exact series hits plus exact map hits: the first tie-break.
    pub fn exact_hits(&self) -> u32 {
        self.series_hits + self.map_hits
    }

    /// Share of decided winner parts that hit; `None` before any is decided.
    pub fn winner_accuracy(&self) -> Option<f64> {
        rate(self.winner_hits, self.winner_decided)
    }

    /// Share of decided series parts with the exact score (T15).
    pub fn series_accuracy(&self) -> Option<f64> {
        rate(self.series_hits, self.series_decided)
    }

    /// Share of decided map winner parts that hit (T15).
    pub fn map_winner_accuracy(&self) -> Option<f64> {
        rate(self.map_winner_hits, self.map_winner_decided)
    }

    /// Share of decided map score parts that hit (T15).
    pub fn map_score_accuracy(&self) -> Option<f64> {
        rate(self.map_hits, self.map_score_decided)
    }

    fn add(&mut self, p: &ScoredPick) {
        self.points += p.points;
        if p.state == ResultState::Void {
            return;
        }
        if p.state == ResultState::Provisional {
            self.provisional_points += p.points;
            self.provisional_picks += 1;
        }
        self.settled_picks += 1;
        let b = &p.breakdown;
        count(b.winner, &mut self.winner_hits, &mut self.winner_decided);
        count(b.series, &mut self.series_hits, &mut self.series_decided);
        for m in &b.maps {
            count(
                Some(m.winner),
                &mut self.map_winner_hits,
                &mut self.map_winner_decided,
            );
            count(m.score, &mut self.map_hits, &mut self.map_score_decided);
        }
    }
}

/// `hits / decided`; `None` before any part is decided.
pub fn rate(hits: u32, decided: u32) -> Option<f64> {
    (decided > 0).then(|| f64::from(hits) / f64::from(decided))
}

/// Counts one part: `hit` and `miss` are decided; `pending`, `void` and a
/// missing part count neither way.
fn count(part: Option<PartScore>, hits: &mut u32, decided: &mut u32) {
    match part.map(|p| p.outcome) {
        Some(Outcome::Hit) => {
            *hits += 1;
            *decided += 1;
        }
        Some(Outcome::Miss) => *decided += 1,
        Some(Outcome::Pending | Outcome::Void) | None => {}
    }
}

/// Totals per member, by user ID. Members without a non-void result are
/// left out.
pub fn tally<'a>(picks: impl IntoIterator<Item = &'a ScoredPick>) -> Vec<Standing> {
    let mut by_user: HashMap<&str, Standing> = HashMap::new();
    for p in picks {
        by_user
            .entry(p.user_id.as_str())
            .or_insert_with(|| Standing::new(&p.user_id))
            .add(p);
    }
    let mut out: Vec<Standing> = by_user
        .into_values()
        .filter(|s| s.settled_picks > 0)
        .collect();
    out.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    out
}

/// The tie-break: `Less` means `a` ranks above `b`. Points, then exact hits,
/// then winner accuracy (as an exact fraction; none decided counts as 0).
/// `Equal` means a shared rank.
pub fn compare(a: &Standing, b: &Standing) -> Ordering {
    b.points
        .cmp(&a.points)
        .then_with(|| b.exact_hits().cmp(&a.exact_hits()))
        .then_with(|| {
            // a_hits / a_decided vs b_hits / b_decided, cross-multiplied.
            let frac = |s: &Standing| {
                if s.winner_decided == 0 {
                    (0, 1)
                } else {
                    (u64::from(s.winner_hits), u64::from(s.winner_decided))
                }
            };
            let (ah, ad) = frac(a);
            let (bh, bd) = frac(b);
            (bh * ad).cmp(&(ah * bd))
        })
}

/// A ranked member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    /// 1-based; members that [`compare`] as equal share a rank, and the next
    /// rank skips (1, 1, 3).
    pub rank: u32,
    pub standing: Standing,
}

/// Ranks `standings` by [`compare`]. Inside a shared rank, members are
/// ordered by display name (case-insensitive, from `names`, else the user
/// ID), then by user ID, so the order is stable.
pub fn rank(standings: Vec<Standing>, names: &HashMap<String, String>) -> Vec<Ranked> {
    let name = |s: &Standing| {
        names
            .get(&s.user_id)
            .map_or_else(|| s.user_id.clone(), |n| n.to_lowercase())
    };
    let mut keyed: Vec<(String, Standing)> = standings.into_iter().map(|s| (name(&s), s)).collect();
    keyed.sort_by(|(an, a), (bn, b)| {
        compare(a, b)
            .then_with(|| an.cmp(bn))
            .then_with(|| a.user_id.cmp(&b.user_id))
    });
    let mut out: Vec<Ranked> = Vec::with_capacity(keyed.len());
    for (i, (_, s)) in keyed.into_iter().enumerate() {
        let rank = match out.last() {
            Some(prev) if compare(&prev.standing, &s) == Ordering::Equal => prev.rank,
            _ => u32::try_from(i + 1).unwrap_or(u32::MAX),
        };
        out.push(Ranked { rank, standing: s });
    }
    out
}

/// One row of the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRow {
    pub rank: u32,
    /// The rank before the latest match day; `None` if the member had no
    /// standing then.
    pub previous_rank: Option<u32>,
    pub standing: Standing,
}

/// The ranked leaderboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub rows: Vec<BoardRow>,
    /// The latest local day with a non-void result; `None` without any.
    pub match_day: Option<NaiveDate>,
    /// Anyone had a standing before the latest match day, so rank changes
    /// mean something.
    pub has_previous: bool,
}

/// Ranks `picks` and compares with the ranking before the latest match day
/// in `tz`. Picks without a match time count as earlier.
pub fn build(picks: &[ScoredPick], tz: Tz, names: &HashMap<String, String>) -> Board {
    let match_day = picks
        .iter()
        .filter(|p| p.state != ResultState::Void)
        .filter_map(|p| p.match_at)
        .max()
        .map(|at| localtime::local_date(tz, at));
    let previous: HashMap<String, u32> = match match_day {
        Some(day) => {
            let (cutoff, _) = localtime::day_window(tz, day);
            let before = picks
                .iter()
                .filter(|p| p.match_at.is_none_or(|at| at < cutoff));
            rank(tally(before), names)
                .into_iter()
                .map(|r| (r.standing.user_id, r.rank))
                .collect()
        }
        None => HashMap::new(),
    };
    let rows = rank(tally(picks), names)
        .into_iter()
        .map(|r| BoardRow {
            rank: r.rank,
            previous_rank: previous.get(&r.standing.user_id).copied(),
            standing: r.standing,
        })
        .collect();
    Board {
        rows,
        match_day,
        has_previous: !previous.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::scoring::MapScore;

    fn part(outcome: Outcome, points: u32) -> PartScore {
        PartScore { outcome, points }
    }

    fn st(user: &str, points: i64, exact: u32, hits: u32, decided: u32) -> Standing {
        Standing {
            points,
            series_hits: exact,
            winner_hits: hits,
            winner_decided: decided,
            settled_picks: 1,
            ..Standing::new(user)
        }
    }

    fn no_names() -> HashMap<String, String> {
        HashMap::new()
    }

    fn ranks(rows: &[Ranked]) -> Vec<(&str, u32)> {
        rows.iter()
            .map(|r| (r.standing.user_id.as_str(), r.rank))
            .collect()
    }

    #[test]
    fn points_decide_first() {
        let r = rank(
            vec![
                st("a", 5, 9, 9, 9),
                st("b", 7, 0, 0, 1),
                st("c", 6, 0, 0, 0),
            ],
            &no_names(),
        );
        assert_eq!(ranks(&r), [("b", 1), ("c", 2), ("a", 3)]);
    }

    #[test]
    fn equal_points_go_to_more_exact_hits() {
        // Series and map hits both count.
        let mut a = st("a", 10, 1, 0, 0);
        a.map_hits = 1;
        let b = st("b", 10, 3, 0, 0);
        let c = st("c", 10, 0, 5, 5);
        let r = rank(vec![a, b, c], &no_names());
        assert_eq!(ranks(&r), [("b", 1), ("a", 2), ("c", 3)]);
    }

    #[test]
    fn equal_hits_go_to_better_winner_accuracy() {
        // 3/4 beats 2/3, which beats 1/2; none decided counts as 0.
        let r = rank(
            vec![
                st("half", 9, 1, 1, 2),
                st("none", 9, 1, 0, 0),
                st("three_q", 9, 1, 3, 4),
                st("two_t", 9, 1, 2, 3),
            ],
            &no_names(),
        );
        assert_eq!(
            ranks(&r),
            [("three_q", 1), ("two_t", 2), ("half", 3), ("none", 4)]
        );
        // Equal fractions with different counts are equal.
        assert_eq!(
            compare(&st("a", 9, 1, 1, 2), &st("b", 9, 1, 2, 4)),
            Ordering::Equal
        );
        // No decided winner equals 0 of n.
        assert_eq!(
            compare(&st("a", 9, 1, 0, 0), &st("b", 9, 1, 0, 3)),
            Ordering::Equal
        );
    }

    #[test]
    fn full_ties_share_a_rank_and_the_next_rank_skips() {
        let r = rank(
            vec![
                st("d", 4, 0, 0, 0),
                st("b", 9, 1, 2, 3),
                st("a", 9, 1, 2, 3),
                st("c", 9, 1, 4, 6),
                st("e", 4, 0, 0, 0),
            ],
            &no_names(),
        );
        assert_eq!(
            ranks(&r),
            [("a", 1), ("b", 1), ("c", 1), ("d", 4), ("e", 4)]
        );
    }

    #[test]
    fn shared_ranks_are_ordered_by_name_then_id() {
        let names: HashMap<String, String> = [
            ("1".to_owned(), "zed".to_owned()),
            ("2".to_owned(), "Amy".to_owned()),
            ("3".to_owned(), "amy".to_owned()),
        ]
        .into();
        let r = rank(
            vec![
                st("1", 3, 0, 0, 0),
                st("3", 3, 0, 0, 0),
                st("2", 3, 0, 0, 0),
            ],
            &names,
        );
        assert_eq!(ranks(&r), [("2", 1), ("3", 1), ("1", 1)]);
    }

    fn pick(user: &str, points: i64, state: ResultState, breakdown: Breakdown) -> ScoredPick {
        ScoredPick {
            user_id: user.into(),
            points,
            state,
            breakdown,
            match_at: None,
        }
    }

    fn bd(winner: Outcome, series: Option<Outcome>, map_scores: &[Option<Outcome>]) -> Breakdown {
        Breakdown {
            winner: Some(part(winner, 3)),
            series: series.map(|o| part(o, 3)),
            maps: map_scores
                .iter()
                .enumerate()
                .map(|(i, s)| MapScore {
                    map_number: u8::try_from(i + 1).unwrap(),
                    winner: part(Outcome::Hit, 1),
                    score: s.map(|o| part(o, 2)),
                })
                .collect(),
        }
    }

    #[test]
    fn tally_counts_parts_by_outcome() {
        use Outcome::*;
        let picks = [
            pick(
                "a",
                12,
                ResultState::Final,
                bd(Hit, Some(Hit), &[Some(Hit), Some(Miss), None]),
            ),
            pick("a", 1, ResultState::Final, bd(Miss, Some(Miss), &[])),
            // Pending and void parts count neither way.
            pick(
                "a",
                2,
                ResultState::Final,
                bd(Pending, None, &[Some(Pending)]),
            ),
            pick(
                "a",
                0,
                ResultState::Final,
                bd(Void, Some(Void), &[Some(Void)]),
            ),
            // Live: points count and are marked provisional.
            pick(
                "a",
                3,
                ResultState::Provisional,
                bd(Pending, Some(Pending), &[Some(Hit)]),
            ),
        ];
        let t = tally(&picks);
        assert_eq!(
            t,
            [Standing {
                user_id: "a".into(),
                points: 18,
                provisional_points: 3,
                provisional_picks: 1,
                settled_picks: 5,
                winner_hits: 1,
                winner_decided: 2,
                series_hits: 1,
                series_decided: 2,
                // `bd` gives every map a winner hit: 3 + 1 + 1 + 1 maps.
                map_winner_hits: 6,
                map_winner_decided: 6,
                map_hits: 2,
                map_score_decided: 3,
            }]
        );
        assert_eq!(t[0].exact_hits(), 3);
        assert_eq!(t[0].winner_accuracy(), Some(0.5));
        assert_eq!(t[0].series_accuracy(), Some(0.5));
        assert_eq!(t[0].map_winner_accuracy(), Some(1.0));
        assert_eq!(t[0].map_score_accuracy(), Some(2.0 / 3.0));
    }

    #[test]
    fn void_results_count_nothing_and_void_only_members_are_left_out() {
        use Outcome::*;
        let picks = [
            pick("a", 0, ResultState::Void, bd(Void, None, &[])),
            pick("b", 0, ResultState::Void, bd(Void, None, &[])),
            pick("b", 3, ResultState::Final, bd(Hit, None, &[])),
        ];
        let t = tally(&picks);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].user_id, "b");
        assert_eq!(t[0].settled_picks, 1);
        assert_eq!(t[0].points, 3);
        assert_eq!(Standing::new("x").winner_accuracy(), None);
    }

    #[test]
    fn accuracy_per_part_skips_pending_void_parts_and_void_results() {
        use Outcome::*;
        let maps = |winners: &[Outcome]| {
            winners
                .iter()
                .enumerate()
                .map(|(i, &w)| MapScore {
                    map_number: u8::try_from(i + 1).unwrap(),
                    winner: part(w, 1),
                    score: None,
                })
                .collect::<Vec<_>>()
        };
        let picks = [
            pick(
                "a",
                1,
                ResultState::Final,
                Breakdown {
                    winner: None,
                    series: None,
                    maps: maps(&[Hit, Miss, Pending, Void]),
                },
            ),
            // A void result counts for nothing, even with hits in it.
            pick(
                "a",
                0,
                ResultState::Void,
                Breakdown {
                    winner: Some(part(Hit, 3)),
                    series: Some(part(Hit, 3)),
                    maps: maps(&[Hit, Hit]),
                },
            ),
        ];
        let t = tally(&picks);
        let s = &t[0];
        assert_eq!((s.map_winner_hits, s.map_winner_decided), (1, 2));
        assert_eq!(s.map_winner_accuracy(), Some(0.5));
        assert_eq!((s.winner_hits, s.winner_decided), (0, 0));
        assert_eq!((s.series_hits, s.series_decided), (0, 0));
        assert_eq!(s.winner_accuracy(), None);
        assert_eq!(s.series_accuracy(), None);
        assert_eq!(s.map_score_accuracy(), None);
        assert_eq!(s.settled_picks, 1);
        assert_eq!(rate(0, 0), None);
        assert_eq!(rate(3, 4), Some(0.75));
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
    }

    fn dated(user: &str, points: i64, at: DateTime<Utc>) -> ScoredPick {
        ScoredPick {
            match_at: Some(at),
            ..pick(
                user,
                points,
                ResultState::Final,
                bd(Outcome::Miss, None, &[]),
            )
        }
    }

    #[test]
    fn one_match_day_has_no_previous_ranks() {
        let berlin = chrono_tz::Europe::Berlin;
        let b = build(
            &[
                dated("a", 3, utc(2026, 11, 20, 12)),
                dated("b", 6, utc(2026, 11, 20, 15)),
            ],
            berlin,
            &no_names(),
        );
        assert_eq!(b.match_day, NaiveDate::from_ymd_opt(2026, 11, 20));
        assert!(!b.has_previous);
        assert!(b.rows.iter().all(|r| r.previous_rank.is_none()));
        assert_eq!(build(&[], berlin, &no_names()).match_day, None);
    }

    #[test]
    fn previous_rank_leaves_out_the_latest_match_day() {
        let berlin = chrono_tz::Europe::Berlin;
        let picks = [
            // Day 1 (20 Nov): a leads b.
            dated("a", 6, utc(2026, 11, 20, 12)),
            dated("b", 3, utc(2026, 11, 20, 12)),
            // Day 2 (21 Nov local, 23:30 UTC on the 20th is already the
            // 21st in Berlin): b overtakes, c joins.
            dated("b", 6, utc(2026, 11, 20, 23)),
            dated("c", 1, utc(2026, 11, 21, 18)),
            // A void match later still doesn't make a match day.
            ScoredPick {
                state: ResultState::Void,
                ..dated("a", 0, utc(2026, 11, 25, 12))
            },
        ];
        let b = build(&picks, berlin, &no_names());
        assert_eq!(b.match_day, NaiveDate::from_ymd_opt(2026, 11, 21));
        assert!(b.has_previous);
        let rows: Vec<_> = b
            .rows
            .iter()
            .map(|r| (r.standing.user_id.as_str(), r.rank, r.previous_rank))
            .collect();
        assert_eq!(rows, [("b", 1, Some(2)), ("a", 2, Some(1)), ("c", 3, None)]);
    }

    #[test]
    fn match_day_follows_the_local_calendar_across_dst() {
        let berlin = chrono_tz::Europe::Berlin;
        // Summer (UTC+2): 22:30 UTC on 1 Jul is 2 Jul in Berlin.
        let b = build(
            &[
                dated("a", 1, utc(2026, 7, 1, 21)),
                dated("a", 1, utc(2026, 7, 1, 22)),
            ],
            berlin,
            &no_names(),
        );
        assert_eq!(b.match_day, NaiveDate::from_ymd_opt(2026, 7, 2));
        assert!(b.has_previous, "21:00 UTC is still 1 Jul in Berlin");
        // Winter (UTC+1): 22:00 UTC is 23:00, the same day.
        let b = build(
            &[
                dated("a", 1, utc(2026, 11, 1, 21)),
                dated("a", 1, utc(2026, 11, 1, 22)),
            ],
            berlin,
            &no_names(),
        );
        assert_eq!(b.match_day, NaiveDate::from_ymd_opt(2026, 11, 1));
        assert!(!b.has_previous);
    }
}
