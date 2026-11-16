# P2 – Crowd insight and upset bonus

- **Epic:** Proposed extras (optional)
- **Status:** proposed (optional)
- **Index:** [all tickets](README.md)

**Goal:** show what the server thinks, and reward correct calls against the crowd.

**Depends on:** T10, T11, T12

**Scope**

- **Crowd favourite:** the team with the larger share of winner picks, shown on match rows and in the detail, e.g. "62% pick Team A". A winner set implicitly by a series score counts. A tie means no favourite.
- **Pick stats snapshot at lock:** when a match locks, store the winner shares and number of pickers in `match_pick_stats`. An admin reopen discards the snapshot, and the next lock takes a new one.
- **Upset bonus** (guild setting, **off by default**): a correct winner pick earns +2 points if, in the snapshot, that team had **less than 25%** of winner picks and at least **4** members picked a winner. All three numbers are configurable.
- Part of `score()` (T11) as breakdown line `upset_bonus`, included in the rules version. A setting change applies only to matches that lock afterwards.
- The result post (T08) names the upset callers. Leaderboard and profile get an *upsets called* stat.

**Acceptance criteria**

- [ ] Boundary tests: 24.9% earns the bonus, 25.0% does not; 3 pickers never earn it.
- [ ] Picks edited before the lock change the favourite; nothing changes the snapshot after the lock.
- [ ] With the bonus off, points equal T11 exactly.
