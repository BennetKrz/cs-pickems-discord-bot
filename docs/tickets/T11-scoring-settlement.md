# T11 – Scoring and settlement

- **Epic:** C – Picks and scoring
- **Status:** in review
- **Index:** [all tickets](README.md)

**Goal:** points are always correct, including after corrections, and they move while a match is running.

**Depends on:** T10, T03 (domain events), T05 (void)

**Default scoring rules** (per guild, stored as JSON with a version number)

| Part | Condition | Points |
| --- | --- | --- |
| Winner | correct | 3 |
| Series score (Bo3/Bo5) | exact | 3 |
| Map winner | correct, per map | 1 |
| Map score | exact regulation score, or correct *overtime win* team, per map | 2 more |

Maximum per match: Bo1 6, Bo3 15, Bo5 21. Longer series weigh more, which suits playoffs and finals. *Default values chosen during planning; the owner may change them.*

**Scope**

- `score(pick, result, rules) -> (points, breakdown)` as a pure function in `domain`.
- **Settlement is a recompute, not a delta:** for an affected match, recompute every pick and upsert `pick_results`. The leaderboard only reads `pick_results`. That makes corrections and replays safe.
- **Triggers:** `MapUpdated` with a final map score settles that map's part as *provisional*. `MatchFinished` settles everything and marks it final. `ResultCorrected`, void and reopen re-run the match.
- **Rules snapshot:** a match uses the rules version active when it locked, so a rule change mid-event does not reshuffle history. `app rescore --all` applies current rules on purpose.
- **Void cases:** canceled match, forfeit, or admin void gives 0 points and is excluded from accuracy stats.
- **Missing map round scores** (possible on the PandaScore free plan, T04): the map winner part settles; the exact-score part stays pending. Without data after 48 h it becomes void.

**Acceptance criteria**

- [x] Unit tests cover every rule, including overtime, unplayed maps and void cases.
- [x] A corrected result changes points and leaderboard; running settlement twice changes nothing.
- [x] Provisional map points appear while the match is live and become final at match end.
- [x] Settling a match with 50 picks takes under 1 s on a Raspberry Pi 4. *(Asserted in `sync/tests/settle.rs`; measured at ~15 ms debug and ~3.5 ms release on a dev machine. Not yet timed on a real Pi 4.)*

**Notes**

- Decisions are in [ADR 0011](../adr/0011-scoring-and-settlement.md). The owner chose:
  - the branch stacks on T10 (still in review);
  - T11 includes a way to change rules: `store::scoring::set_rules` plus `app rules show|set --guild <id>`;
  - the 48 h data deadline starts at match end (`ended_at`, else when settlement first saw the match finished).
- Also decided:
  - every domain event re-settles its match from the current state; a sweep (at start, on lag, every `SYNC__SETTLE_EVERY`, default 10 min) catches missed events and voids expired pending parts;
  - a finished match's results are `final` even while map-score parts wait for round scores; `pending_parts` counts them;
  - overtime = both teams reached 12 rounds and the winner has more than 13;
  - a result row is only rewritten when something besides `settled_at` changed, so a second run writes nothing;
  - a reopened (open again) match loses its results and rules pins.
- Code:
  - rules: `backend/crates/domain/src/scoring.rs`, plus `ResultState` in `domain::model`;
  - migration `backend/crates/store/migrations/0005_scoring.sql`;
  - store: `store::scoring` (rules versions, pins, `totals_for_guild`, `matches_to_settle`, `matches_with_picks`); `store::picks::{list_for_match_all_guilds, results_for_match, delete_results_for_match}`; `PickResult` gains `state` and `pending_parts`;
  - worker: `backend/crates/sync/src/settle.rs` (`settle_match`, `sweep`, `rescore_all`, `run_settler`), wired in `app::run`;
  - CLI: `app rescore --all`, `app rules show --guild <id>`, `app rules set --guild <id> <file.json>`;
  - config: `settle_every` / `SYNC__SETTLE_EVERY`.
- Tests:
  - `domain::scoring` (every part in Bo1/Bo3/Bo5, max points, overtime, unplayed maps, missing rounds pending then void, live, canceled/forfeit/voided, rules JSON);
  - `store/tests/repos.rs` (rules history and `version_at` boundaries, pins, the sweep query, totals);
  - `sync/tests/settle.rs`:
    - provisional then final through the fixture scenario;
    - a manual series correction changes points and totals;
    - settling twice writes nothing;
    - void/unvoid, cancel, reopen;
    - rules pinned at lock and `rescore_all`;
    - the 48 h void;
    - the worker's start-up sweep and event handling;
    - 50 picks in under 1 s;
  - `app/tests/process.rs` (`rules show|set`, `rescore --all`).
- Checked by hand on a dev build in fixture mode:
  - a full Bo3 pick through `PUT /api/matches/5/pick`;
  - advancing the dev clock gave 0 → 3 → 6 provisional, then 15 final;
  - `app rules set` (winner 5), then `app rescore --all`, gave 17 under rules version 2.
- Also fixed: the T10 commit had garbled the header comment of `store/tests/repos.rs`; it is restored.
- Follow-ups:
  - **T12/T15:** show the breakdown from `pick_results`; count accuracy over `hit`/`miss` parts only (`void` and `pending` are excluded).
  - **T14:** build the leaderboard on `store::scoring::totals_for_guild` (reads `pick_results` only); decide whether provisional points show.
  - Time `settle_match` with 50 picks on the Pi once it is set up.
