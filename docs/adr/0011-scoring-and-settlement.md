# 0011 – Scoring and settlement

- **Status:** accepted
- **Date:** 2026-11-21
- **Ticket:** T11

## Context

T11 turns results into points. The ticket fixes the default points, "settlement is a recompute, not a delta", the triggers, the rules snapshot at lock time, the void cases and the pending-then-void rule for missing round scores. Several details were open:

- **Rule changes.** `guild_settings` (T02) holds only the current rules and version, and nothing wrote them. A snapshot at lock time needs the old versions too.
- **48 h start.** The ticket doesn't say from when the 48 h count.
- **Provisional vs. final.** A finished match can still have parts waiting for round scores. On the PandaScore free plan that is every map-score pick (ADR 0001).
- **Overtime.** The ticket names an *overtime win* bucket (T10) but doesn't define when a map counts as overtime.
- **Idempotence.** The acceptance criterion says "running settlement twice changes nothing", but `pick_results.settled_at` would change on every rewrite.
- **Triggers.** Settlement must run on `MapUpdated`, `MatchFinished`, `ResultCorrected`, void and reopen. Some events can be lost: published before a subscriber started, or dropped by a lagging broadcast receiver.

## Decision

1. **`domain::scoring` is pure.**
   - `ScoringRules { winner, series, map_winner, map_score }`:
     - JSON with unknown fields rejected; a missing field takes its default (3, 3, 1, 2);
     - at most 100 per part.
   - `score(pick, result, rules)` returns points, a `ResultState` (`provisional` / `final` / `void`), a count of pending parts, and a breakdown. In the breakdown, every part has an outcome (`hit` / `miss` / `pending` / `void`) and its points.
   - `MatchResult::from_match` builds the result view. A canceled match (walkovers included, ADR 0001) and a voided one are `Void`. A scheduled or postponed match is `Open`.
2. **Part rules.**
   - Winner and series are `pending` while live. When finished, they compare with the stored winner, or the side whose `maps_won` reached the wins needed, and with `maps_won`.
   - A map pick is void if the map was never played: status `not_played`, or a number beyond `maps_won_a + maps_won_b` of a decided, finished series.
   - A finished map settles its winner part (the stored winner, else the side with more rounds), and its score part if rounds are known. A map that hasn't finished is pending.
   - **Overtime** means both teams reached 12 rounds and the winner has more than 13. An overtime pick hits if the map went to overtime and the picked team won it. A regulation pick hits on the exact score.
   - **Missing data** (no rounds, no map winner, no decided series on a finished match): the part is `pending`, and `void` once the data deadline passed.
3. **The data deadline is 48 h after match end**: `matches.ended_at`, or, if that is NULL, when settlement first saw the match finished (`match_scoring.finished_seen_at`). It only applies to finished matches.
4. **States.** Live gives `provisional`, finished gives `final`, canceled or voided gives `void`. A `final` result can still have `pending_parts > 0` until round scores arrive or the deadline passes. "Final" means the match is over; it doesn't mean every part is decided. `void` parts and `void` results don't count for accuracy (T15).
5. **Rules versions and pins** (migration `0005_scoring.sql`):
   - `scoring_rule_versions (guild_id, version, rules, valid_from)` is append-only.
     - `store::scoring::set_rules` writes a new version, valid from now, and updates `guild_settings`.
     - On the first change, the old version is written too, valid since 1970.
     - NULL rules mean the defaults.
   - `match_scoring (guild_id, match_id, rules_version, finished_seen_at)` pins the version per guild and match at its first settlement. The pinned version is the one valid at the lock moment, `min(lock_at, started_at)`.
     - `app rescore --all` re-pins every match to the current version on purpose.
     - A match that is open again (reopen) loses its pins and results, so its next lock picks the rules anew.
   - CLI: `app rules show --guild <id>` (JSON with defaults filled in, plus history) and `app rules set --guild <id> <file.json>` (validated first).
6. **Settlement** (`sync::settle`):
   - `settle_match` recomputes every pick of the match in all guilds, in one `BEGIN IMMEDIATE` transaction.
   - A row is written only when points, breakdown, state, pending count or rules version differ. A second run writes nothing, `settled_at` included.
   - `pick_results` gets `state` and `pending_parts` columns. `store::scoring::totals_for_guild` sums `pick_results` per member; T14 builds on it.
7. **Triggers: every domain event re-settles its match.** The recompute derives everything from the current state, so the ticket's triggers are covered, and other events are harmless.
   - The worker (`run_settler`, in the sync task on the app clock) subscribes before anything publishes.
   - It also runs a **sweep** of `store::scoring::matches_to_settle`:
     - when to sweep: at start, after a lagged receiver, and every `settle_every` (default 10 min, `SYNC__SETTLE_EVERY`);
     - what it finds: picks without results on started, canceled or voided matches; provisional results; pending parts; results whose state no longer fits the match; results on a match that is open again.
   - The sweep catches up on missed events and turns expired pending parts into void.

## Consequences

- Corrections, replays (`app reconcile --replay` publishes to no consumer) and restarts converge on the same `pick_results` at the next event or sweep.
- On the PandaScore free plan, map-score parts stay pending for 48 h and then become void, unless an admin enters the rounds (T05). That re-settles the match through `MapUpdated`.
- While a match is live, the sweep recomputes it every `settle_every` without writing anything, unless something changed. That is cheap: settling 50 full Bo5 picks takes about 15 ms in a debug build on a dev machine (asserted < 1 s in `sync/tests/settle.rs`).
- Changing rules never touches settled history unless `app rescore --all` is run.
- The breakdown JSON is not in the API yet; T12 (match detail) and T15 (profile, accuracy) read it from `pick_results`.
- `store::picks::list_for_match_all_guilds`, `store::scoring::version_at` and `rules_of` take a connection or executor rather than `Acquire`. Settlement runs them inside its own transaction in a spawned task, which needs a `Send` future.
