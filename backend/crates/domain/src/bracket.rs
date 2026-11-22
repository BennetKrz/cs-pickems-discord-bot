//! Stage structure for the event view (T13): bracket positions of
//! elimination matches, round numbers of Swiss matches, and the standings
//! of Swiss and group stages. Pure; see
//! `docs/adr/0017-event-view-and-brackets.md`.
//!
//! - **Positions.** Stored values (`bracket_side`, `round_index`,
//!   `slot_index`) win. PandaScore only sets the side and the feeds, so the
//!   rest is derived from the feeds:
//!   - side: fed by any loser → lower; fed by winners of both the upper
//!     and the lower bracket (or of a final) → final; fed by lower winners
//!     → lower; else upper;
//!   - round: 1 + the highest round among winner feeders on the same side,
//!     else 0;
//!   - slot: the rank within its column (side and round), by stored slot,
//!     else the mean slot of its feeders, then start time, then ID.
//!
//!   Without any links or positions every match lands in one upper column
//!   by start time: the ticket's fallback.
//! - **Swiss rounds** come from `round_index` when set, else from how many
//!   earlier matches the two teams played in the stage.
//! - **Standings** count finished matches with a winner: wins, losses, map
//!   difference and round difference, in that order of precedence.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::model::{
    BracketSide, Feed, FeedOutcome, MapStatus, Match, MatchId, MatchMap, MatchStatus, TeamId,
};

/// Where a match sits in an elimination tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub side: BracketSide,
    /// Column within the side, from 0.
    pub round: u32,
    /// Row within the column, from 0, without gaps.
    pub slot: u32,
    /// Side, round or slot was derived rather than stored.
    pub derived: bool,
}

/// The feeds of `m` that point at another match of `ids`.
fn feeds_in(m: &Match, ids: &HashSet<MatchId>) -> Vec<Feed> {
    let b = &m.fields.bracket;
    [b.feeds_a_from, b.feeds_b_from]
        .into_iter()
        .flatten()
        .filter(|f| f.match_id != m.id && ids.contains(&f.match_id))
        .collect()
}

/// The stage has a bracket: a side or a feed between its own matches.
pub fn has_bracket_data(matches: &[Match]) -> bool {
    let ids: HashSet<MatchId> = matches.iter().map(|m| m.id).collect();
    matches
        .iter()
        .any(|m| m.fields.bracket.side.is_some() || !feeds_in(m, &ids).is_empty())
}

struct Resolver<'a> {
    by_id: HashMap<MatchId, &'a Match>,
    ids: HashSet<MatchId>,
    sides: HashMap<MatchId, BracketSide>,
    rounds: HashMap<MatchId, u32>,
    /// Matches whose side or round is being resolved, to stop at cycles.
    visiting_side: HashSet<MatchId>,
    visiting_round: HashSet<MatchId>,
}

impl Resolver<'_> {
    fn side(&mut self, id: MatchId) -> BracketSide {
        if let Some(s) = self.sides.get(&id) {
            return *s;
        }
        let m = self.by_id[&id];
        let side = if let Some(s) = m.fields.bracket.side {
            s
        } else if !self.visiting_side.insert(id) {
            // A cycle in the feeds; a valid bracket has none.
            return BracketSide::Upper;
        } else {
            let feeds = feeds_in(m, &self.ids);
            let side = if feeds.iter().any(|f| f.outcome == FeedOutcome::Loser) {
                BracketSide::Lower
            } else {
                let from: Vec<BracketSide> = feeds.iter().map(|f| self.side(f.match_id)).collect();
                let has = |s| from.contains(&s);
                if has(BracketSide::Final) || (has(BracketSide::Upper) && has(BracketSide::Lower)) {
                    BracketSide::Final
                } else if has(BracketSide::Lower) {
                    BracketSide::Lower
                } else {
                    BracketSide::Upper
                }
            };
            self.visiting_side.remove(&id);
            side
        };
        self.sides.insert(id, side);
        side
    }

    fn round(&mut self, id: MatchId) -> u32 {
        if let Some(r) = self.rounds.get(&id) {
            return *r;
        }
        let m = self.by_id[&id];
        let round = if let Some(r) = m.fields.bracket.round_index {
            u32::try_from(r).unwrap_or(0)
        } else if !self.visiting_round.insert(id) {
            return 0;
        } else {
            let side = self.side(id);
            let mut round = 0;
            for f in feeds_in(m, &self.ids) {
                if f.outcome == FeedOutcome::Winner && self.side(f.match_id) == side {
                    round = round.max(self.round(f.match_id) + 1);
                }
            }
            self.visiting_round.remove(&id);
            round
        };
        self.rounds.insert(id, round);
        round
    }
}

fn side_order(s: BracketSide) -> u8 {
    match s {
        BracketSide::Upper => 0,
        BracketSide::Lower => 1,
        BracketSide::Final => 2,
    }
}

/// Bracket positions of the matches of one stage. Feeds to matches outside
/// `matches` are ignored.
pub fn positions(matches: &[Match]) -> HashMap<MatchId, Pos> {
    let mut r = Resolver {
        by_id: matches.iter().map(|m| (m.id, m)).collect(),
        ids: matches.iter().map(|m| m.id).collect(),
        sides: HashMap::new(),
        rounds: HashMap::new(),
        visiting_side: HashSet::new(),
        visiting_round: HashSet::new(),
    };
    // All sides first: a round compares its feeders' sides.
    for m in matches {
        r.side(m.id);
    }
    let mut columns: HashMap<(u8, u32), Vec<&Match>> = HashMap::new();
    for m in matches {
        let side = r.sides[&m.id];
        let round = r.round(m.id);
        columns
            .entry((side_order(side), round))
            .or_default()
            .push(m);
    }
    let mut keys: Vec<(u8, u32)> = columns.keys().copied().collect();
    keys.sort_unstable();

    // Columns in order (upper, lower, final; rounds ascending), so feeders'
    // slots are known when their targets are ranked.
    let mut slots: HashMap<MatchId, u32> = HashMap::new();
    for key in keys {
        let column = &columns[&key];
        let mut ranked: Vec<(Option<f64>, &Match)> = column
            .iter()
            .map(|m| (slot_key(m, &r, &slots), *m))
            .collect();
        ranked.sort_by(|(ka, a), (kb, b)| {
            cmp_opt(ka.as_ref(), kb.as_ref(), f64::total_cmp)
                .then_with(|| {
                    cmp_opt(
                        a.fields.scheduled_at.as_ref(),
                        b.fields.scheduled_at.as_ref(),
                        Ord::cmp,
                    )
                })
                .then(a.id.cmp(&b.id))
        });
        for (i, (_, m)) in ranked.into_iter().enumerate() {
            slots.insert(m.id, u32::try_from(i).unwrap_or(u32::MAX));
        }
    }

    matches
        .iter()
        .map(|m| {
            let b = &m.fields.bracket;
            let pos = Pos {
                side: r.sides[&m.id],
                round: r.rounds[&m.id],
                slot: slots[&m.id],
                derived: b.side.is_none() || b.round_index.is_none() || b.slot_index.is_none(),
            };
            (m.id, pos)
        })
        .collect()
}

/// The sort key within a column: the stored slot, else the mean slot of
/// the feeders on the same side, else of all feeders.
fn slot_key(m: &Match, r: &Resolver<'_>, slots: &HashMap<MatchId, u32>) -> Option<f64> {
    if let Some(s) = m.fields.bracket.slot_index {
        return Some(s as f64);
    }
    let side = r.sides[&m.id];
    let feeds = feeds_in(m, &r.ids);
    let mean = |same_side: bool| {
        let known: Vec<f64> = feeds
            .iter()
            .filter(|f| !same_side || r.sides.get(&f.match_id) == Some(&side))
            .filter_map(|f| slots.get(&f.match_id).map(|s| f64::from(*s)))
            .collect();
        (!known.is_empty()).then(|| known.iter().sum::<f64>() / known.len() as f64)
    };
    mean(true).or_else(|| mean(false))
}

/// `None` sorts last.
fn cmp_opt<T>(a: Option<&T>, b: Option<&T>, cmp: impl Fn(&T, &T) -> Ordering) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => cmp(a, b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Matches in schedule order: by start time (unscheduled last), then ID.
fn schedule_order(matches: &[Match]) -> Vec<&Match> {
    let mut sorted: Vec<&Match> = matches.iter().collect();
    sorted.sort_by(|a, b| {
        cmp_opt(
            a.fields.scheduled_at.as_ref(),
            b.fields.scheduled_at.as_ref(),
            Ord::cmp,
        )
        .then(a.id.cmp(&b.id))
    });
    sorted
}

/// Round numbers (from 1) of the matches of a Swiss or group stage.
///
/// `round_index` wins when set (`round_index + 1`). Else, in schedule
/// order, a match is 1 + the most matches either of its known teams played
/// before it in the stage; canceled matches count, as they took a round.
/// With both teams TBD it is 1 + the highest round of earlier matches with
/// a known team.
pub fn swiss_rounds(matches: &[Match]) -> HashMap<MatchId, u32> {
    let mut played: HashMap<TeamId, u32> = HashMap::new();
    let mut highest_known = 0;
    let mut out = HashMap::new();
    for m in schedule_order(matches) {
        let f = &m.fields;
        let teams: Vec<TeamId> = [f.team_a_id, f.team_b_id].into_iter().flatten().collect();
        let round = match f.bracket.round_index {
            Some(r) => u32::try_from(r).unwrap_or(0) + 1,
            None if teams.is_empty() => highest_known + 1,
            None => {
                teams
                    .iter()
                    .map(|t| played.get(t).copied().unwrap_or(0))
                    .max()
                    .unwrap_or(0)
                    + 1
            }
        };
        if !teams.is_empty() {
            highest_known = highest_known.max(round);
        }
        for t in teams {
            *played.entry(t).or_default() += 1;
        }
        out.insert(m.id, round);
    }
    out
}

/// One team's record in a Swiss or group stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRecord {
    pub team_id: TeamId,
    pub wins: u32,
    pub losses: u32,
    /// Maps won minus maps lost.
    pub map_diff: i32,
    /// Rounds won minus rounds lost, over finished maps with round scores.
    pub round_diff: i32,
}

/// The standings of a Swiss or group stage: every team that appears in
/// `matches`, ordered by wins (desc), losses (asc), map difference (desc),
/// round difference (desc), then `tie` (for example by name).
///
/// Only `finished` matches with a winner count, voided ones too: a void
/// cancels points, not the result. `maps` may hold maps of other matches.
pub fn standings(
    matches: &[Match],
    maps: &[MatchMap],
    tie: impl Fn(TeamId, TeamId) -> Ordering,
) -> Vec<StageRecord> {
    let mut records: Vec<StageRecord> = Vec::new();
    let mut index: HashMap<TeamId, usize> = HashMap::new();
    let mut entry = |records: &mut Vec<StageRecord>, t: TeamId| -> usize {
        *index.entry(t).or_insert_with(|| {
            records.push(StageRecord {
                team_id: t,
                wins: 0,
                losses: 0,
                map_diff: 0,
                round_diff: 0,
            });
            records.len() - 1
        })
    };
    for m in schedule_order(matches) {
        let f = &m.fields;
        let a = f.team_a_id.map(|t| entry(&mut records, t));
        let b = f.team_b_id.map(|t| entry(&mut records, t));
        let (Some(a), Some(b), Some(winner)) = (a, b, f.winner_team_id) else {
            continue;
        };
        if f.status != MatchStatus::Finished {
            continue;
        }
        let (rounds_a, rounds_b) = maps
            .iter()
            .filter(|mp| mp.match_id == m.id && mp.status == MapStatus::Finished)
            .filter_map(|mp| mp.rounds_a.zip(mp.rounds_b))
            .fold((0_i32, 0_i32), |(x, y), (ra, rb)| {
                (x + i32::from(ra), y + i32::from(rb))
            });
        let maps_diff = i32::from(f.maps_won_a) - i32::from(f.maps_won_b);
        let (winner_idx, loser_idx) = if Some(winner) == f.team_a_id {
            (a, b)
        } else {
            (b, a)
        };
        records[winner_idx].wins += 1;
        records[loser_idx].losses += 1;
        records[a].map_diff += maps_diff;
        records[b].map_diff -= maps_diff;
        records[a].round_diff += rounds_a - rounds_b;
        records[b].round_diff -= rounds_a - rounds_b;
    }
    records.sort_by(|x, y| {
        y.wins
            .cmp(&x.wins)
            .then(x.losses.cmp(&y.losses))
            .then(y.map_diff.cmp(&x.map_diff))
            .then(y.round_diff.cmp(&x.round_diff))
            .then_with(|| tie(x.team_id, y.team_id))
    });
    records
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, TimeZone, Utc};

    use super::*;
    use crate::model::{BracketPos, EventId, MatchFields, MatchFormat};

    fn t(minutes: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, 10, 0, 0).unwrap() + Duration::minutes(minutes)
    }

    fn feed(id: i64, outcome: FeedOutcome) -> Option<Feed> {
        Some(Feed {
            match_id: MatchId(id),
            outcome,
        })
    }

    fn w(id: i64) -> Option<Feed> {
        feed(id, FeedOutcome::Winner)
    }

    fn l(id: i64) -> Option<Feed> {
        feed(id, FeedOutcome::Loser)
    }

    fn m(id: i64, start: i64, bracket: BracketPos) -> Match {
        let mut fields =
            MatchFields::scheduled(EventId(1), None, None, MatchFormat::Bo3, Some(t(start)));
        fields.bracket = bracket;
        Match {
            id: MatchId(id),
            fields,
        }
    }

    fn linked(a: Option<Feed>, b: Option<Feed>) -> BracketPos {
        BracketPos {
            feeds_a_from: a,
            feeds_b_from: b,
            ..BracketPos::default()
        }
    }

    fn stored(side: BracketSide, round: i64, slot: i64, bp: BracketPos) -> BracketPos {
        BracketPos {
            side: Some(side),
            round_index: Some(round),
            slot_index: Some(slot),
            ..bp
        }
    }

    /// The fixture scenario's playoff: 1, 2 upper semis; 3 upper final;
    /// 4 lower round 1; 5 lower final; 6 grand final.
    fn fixture_playoff(with_positions: bool) -> Vec<Match> {
        use BracketSide::*;
        let p = |side, round, slot, bp: BracketPos| {
            if with_positions {
                stored(side, round, slot, bp)
            } else if side == Final {
                // PandaScore knows the side from the match name only.
                BracketPos {
                    side: Some(Final),
                    ..bp
                }
            } else {
                bp
            }
        };
        vec![
            m(1, 0, p(Upper, 0, 0, BracketPos::default())),
            m(2, 0, p(Upper, 0, 1, BracketPos::default())),
            m(3, 240, p(Upper, 1, 0, linked(w(1), w(2)))),
            m(4, 240, p(Lower, 0, 0, linked(l(1), l(2)))),
            m(5, 480, p(Lower, 1, 0, linked(l(3), w(4)))),
            m(6, 720, p(Final, 2, 0, linked(w(3), w(5)))),
        ]
    }

    fn cols(pos: &HashMap<MatchId, Pos>) -> Vec<(i64, BracketSide, u32, u32)> {
        let mut v: Vec<_> = pos
            .iter()
            .map(|(id, p)| (id.0, p.side, p.round, p.slot))
            .collect();
        v.sort_unstable_by_key(|x| x.0);
        v
    }

    #[test]
    fn fixture_playoff_from_stored_positions_and_from_feeds_alone() {
        use BracketSide::*;
        let stored_pos = positions(&fixture_playoff(true));
        assert!(stored_pos.values().all(|p| !p.derived));
        assert_eq!(
            cols(&stored_pos),
            vec![
                (1, Upper, 0, 0),
                (2, Upper, 0, 1),
                (3, Upper, 1, 0),
                (4, Lower, 0, 0),
                (5, Lower, 1, 0),
                (6, Final, 2, 0),
            ]
        );
        let derived = positions(&fixture_playoff(false));
        assert!(derived.values().all(|p| p.derived));
        let mut expected = cols(&stored_pos);
        // Without a stored round the final starts its own column count.
        expected[5].2 = 0;
        assert_eq!(cols(&derived), expected);
    }

    /// A 16-team double elimination built from feeds only: IDs 1–8 upper
    /// round 1, 9–12 round 2, 13–14 semis, 15 upper final; 16–19 lower
    /// round 1 (losers of 1–8), 20–23 (winners 16–19 vs losers 9–12),
    /// 24–25, 26–27 (vs losers 13–14), 28, 29 (vs loser 15); 30 the final.
    pub(crate) fn de16() -> Vec<Match> {
        let mut v = Vec::new();
        let mut id = 1;
        let mut push = |v: &mut Vec<Match>, round: i64, a: Option<Feed>, b: Option<Feed>| {
            v.push(m(id, round * 60, linked(a, b)));
            id += 1;
        };
        for _ in 0..8 {
            push(&mut v, 0, None, None);
        }
        for i in 0..4 {
            push(&mut v, 1, w(1 + 2 * i), w(2 + 2 * i));
        }
        for i in 0..2 {
            push(&mut v, 2, w(9 + 2 * i), w(10 + 2 * i));
        }
        push(&mut v, 3, w(13), w(14));
        for i in 0..4 {
            push(&mut v, 1, l(1 + 2 * i), l(2 + 2 * i));
        }
        for i in 0..4 {
            push(&mut v, 2, w(16 + i), l(12 - i));
        }
        for i in 0..2 {
            push(&mut v, 3, w(20 + 2 * i), w(21 + 2 * i));
        }
        for i in 0..2 {
            push(&mut v, 4, w(24 + i), l(14 - i));
        }
        push(&mut v, 5, w(26), w(27));
        push(&mut v, 6, w(28), l(15));
        let mut gf = m(id, 420, linked(w(15), w(29)));
        gf.fields.bracket.side = Some(BracketSide::Final);
        v.push(gf);
        v
    }

    #[test]
    fn sixteen_team_double_elimination_from_feeds() {
        use BracketSide::*;
        let pos = positions(&de16());
        let rounds = |side| {
            let mut r: Vec<u32> = pos
                .values()
                .filter(|p| p.side == side)
                .map(|p| p.round)
                .collect();
            r.sort_unstable();
            r.dedup();
            r
        };
        assert_eq!(rounds(Upper), vec![0, 1, 2, 3]);
        assert_eq!(rounds(Lower), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(rounds(Final), vec![0]);
        let at = |id: i64| pos[&MatchId(id)];
        assert_eq!((at(15).side, at(15).round), (Upper, 3));
        assert_eq!((at(29).side, at(29).round), (Lower, 5));
        // Lower round 2 follows its lower feeders' order, not the reversed
        // loser drop-ins.
        assert_eq!(
            (20..=23).map(|i| at(i).slot).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        // Every column's slots are 0..n without gaps or duplicates.
        let mut by_col: HashMap<(u8, u32), Vec<u32>> = HashMap::new();
        for p in pos.values() {
            by_col
                .entry((side_order(p.side), p.round))
                .or_default()
                .push(p.slot);
        }
        for slots in by_col.values_mut() {
            slots.sort_unstable();
            assert_eq!(*slots, (0..slots.len() as u32).collect::<Vec<_>>());
        }
    }

    #[test]
    fn without_links_matches_line_up_by_start_time() {
        let ms = vec![
            m(1, 60, BracketPos::default()),
            m(2, 0, BracketPos::default()),
            m(3, 30, BracketPos::default()),
        ];
        assert!(!has_bracket_data(&ms));
        let pos = positions(&ms);
        let order: Vec<(i64, u32, u32)> = [2, 3, 1]
            .iter()
            .map(|id| (*id, pos[&MatchId(*id)].round, pos[&MatchId(*id)].slot))
            .collect();
        assert_eq!(order, vec![(2, 0, 0), (3, 0, 1), (1, 0, 2)]);
        assert!(pos.values().all(|p| p.side == BracketSide::Upper));

        // Stored rounds alone still make columns.
        let ms = vec![
            m(
                1,
                0,
                BracketPos {
                    round_index: Some(1),
                    ..BracketPos::default()
                },
            ),
            m(
                2,
                0,
                BracketPos {
                    round_index: Some(0),
                    ..BracketPos::default()
                },
            ),
        ];
        let pos = positions(&ms);
        assert_eq!((pos[&MatchId(1)].round, pos[&MatchId(2)].round), (1, 0));
    }

    #[test]
    fn feeds_outside_the_stage_and_cycles_are_ignored() {
        let ms = vec![
            m(1, 0, linked(w(99), None)),
            m(2, 0, linked(w(3), None)),
            m(3, 0, linked(w(2), None)),
        ];
        assert!(has_bracket_data(&ms));
        let pos = positions(&ms);
        assert_eq!(pos.len(), 3);
        assert_eq!(pos[&MatchId(1)].round, 0);
    }

    fn swiss(id: i64, start: i64, a: Option<i64>, b: Option<i64>) -> Match {
        let mut x = m(id, start, BracketPos::default());
        x.fields.team_a_id = a.map(TeamId);
        x.fields.team_b_id = b.map(TeamId);
        x
    }

    #[test]
    fn swiss_rounds_from_team_history() {
        let mut canceled = swiss(4, 60, Some(7), Some(8));
        canceled.fields.status = MatchStatus::Canceled;
        let ms = vec![
            swiss(1, 0, Some(1), Some(2)),
            swiss(2, 0, Some(3), Some(4)),
            swiss(3, 60, Some(5), Some(6)),
            canceled,
            swiss(5, 180, Some(1), Some(4)),
            swiss(6, 180, Some(7), Some(5)),
            // Round 3, teams still TBD.
            swiss(7, 300, None, None),
            swiss(8, 300, Some(1), None),
        ];
        let r = swiss_rounds(&ms);
        let got: Vec<u32> = (1..=8).map(|i| r[&MatchId(i)]).collect();
        assert_eq!(got, vec![1, 1, 1, 1, 2, 2, 3, 3]);

        let mut stored = swiss(9, 0, Some(1), Some(2));
        stored.fields.bracket.round_index = Some(4);
        assert_eq!(swiss_rounds(&[stored])[&MatchId(9)], 5);
    }

    fn finished(id: i64, start: i64, a: i64, b: i64, maps: (u8, u8)) -> Match {
        let mut x = swiss(id, start, Some(a), Some(b));
        x.fields.status = MatchStatus::Finished;
        x.fields.format = if maps.0.max(maps.1) == 1 {
            MatchFormat::Bo1
        } else {
            MatchFormat::Bo3
        };
        x.fields.maps_won_a = maps.0;
        x.fields.maps_won_b = maps.1;
        x.fields.winner_team_id = Some(TeamId(if maps.0 > maps.1 { a } else { b }));
        x
    }

    fn map(match_id: i64, number: u8, rounds: (u16, u16)) -> MatchMap {
        MatchMap {
            match_id: MatchId(match_id),
            number,
            map_name: None,
            status: MapStatus::Finished,
            rounds_a: Some(rounds.0),
            rounds_b: Some(rounds.1),
            winner_team_id: None,
        }
    }

    #[test]
    fn standings_order_by_record_then_maps_then_rounds() {
        // The fixture's Swiss stage: 1 AUR, 2 BAS, 3 CIN, 4 DUN, 5 ECK,
        // 6 FJO, 7 GLA, 8 HAL.
        let mut canceled = swiss(4, 90, Some(4), Some(5));
        canceled.fields.status = MatchStatus::Canceled;
        let ms = vec![
            finished(1, 30, 1, 8, (1, 0)),
            finished(2, 30, 2, 7, (0, 1)),
            finished(3, 90, 3, 6, (1, 0)),
            canceled,
            finished(5, 180, 1, 7, (2, 1)),
            finished(6, 180, 3, 8, (2, 0)),
        ];
        let maps = vec![
            map(1, 1, (13, 9)),
            map(2, 1, (8, 13)),
            map(3, 1, (13, 11)),
            map(5, 1, (13, 7)),
            map(5, 2, (14, 16)),
            map(5, 3, (13, 10)),
            map(6, 1, (13, 4)),
            map(6, 2, (13, 11)),
        ];
        let rows = standings(&ms, &maps, |a, b| a.cmp(&b));
        let got: Vec<(i64, u32, u32, i32, i32)> = rows
            .iter()
            .map(|r| (r.team_id.0, r.wins, r.losses, r.map_diff, r.round_diff))
            .collect();
        assert_eq!(
            got,
            vec![
                (3, 2, 0, 3, 13), // CIN: 13:11, 13:4, 13:11
                (1, 2, 0, 2, 11), // AUR: 13:9, 13:7, 14:16, 13:10
                (7, 1, 1, 0, -2), // GLA: 13:8, then 7:13, 16:14, 10:13
                (4, 0, 0, 0, 0),  // DUN and ECK: canceled
                (5, 0, 0, 0, 0),
                (6, 0, 1, -1, -2),
                (2, 0, 1, -1, -5),
                (8, 0, 2, -3, -15),
            ]
        );
    }

    #[test]
    fn unfinished_matches_do_not_count_and_ties_use_the_tie_break() {
        let mut live = finished(1, 0, 1, 2, (1, 0));
        live.fields.status = MatchStatus::Live;
        let rows = standings(&[live], &[], |a, b| b.cmp(&a));
        let got: Vec<(i64, u32)> = rows.iter().map(|r| (r.team_id.0, r.wins)).collect();
        assert_eq!(got, vec![(2, 0), (1, 0)]);
    }
}
