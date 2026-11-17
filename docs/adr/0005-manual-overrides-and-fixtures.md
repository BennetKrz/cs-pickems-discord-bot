# 0005 – Manual overrides, admin match actions and dev fixtures

- **Status:** accepted
- **Date:** 2026-11-16
- **Ticket:** T05

## Context

T05 adds the following:
- admin overrides for status, schedule, series score and map scores;
- void and reopen;
- a `fixture` source with a fake clock for development.

ADR 0004 already says that `manual` is a source without an adapter. It is first in every precedence group and applies through links with `external_id` = the canonical ID. The ticket leaves these questions open:

- How do admin endpoints authenticate before T07 brings Discord sessions?
- What does "clear" do when no other source has a value?
- Where does a void live, given that the reconciler rewrites match fields?
- How is "now" faked?
- How are dev-only parts kept out of production?

The owner decided four of them in the T05 planning session: pluggable auth, unvoid, a neutral reset, and a cargo feature.

## Decision

1. **The manual service lives in `sync::manual`.**
   - `ManualService` writes one `manual` record per match (a `SrcMatch` in canonical A/B orientation, without teams) plus its link.
   - It writes `audit_log` with the acting Discord user and re-merges the match. All of this happens in one transaction, and the domain events are published after the commit.
   - When the last override is cleared, the record and the link are deleted.
   - The `sources` crate has no `manual` feature (a change from ADR 0004 §10), because there is no adapter to put behind it.
2. **Override rules.**
   - A series score must be possible for the format, and both teams must be known. The winner follows from the score.
   - A finished map needs rounds with a winner. An upcoming or unplayed map has no rounds.
   - A status override without timestamps keeps the current ones, or uses `now` when moving to live or finished.
3. **Clearing without another source resets to neutral** (owner's choice).
   - If, after the re-merge, a cleared field's provenance still names `manual`, no other source has a value.
   - In that case a series score becomes 0:0 with no winner, and a map goes back to `upcoming` with no rounds and no winner. Status and schedule keep their value, because there is no neutral one.
   - The stale `field_sources` row is removed. This is an exception to ADR 0004 §4 ("a group no source has a value for keeps its current value") that applies only to admin clears.
4. **Void is a column, `matches.voided`** (migration 0003), and it is reversible (owner's choice: unvoid).
   - `MatchFields.voided` is carried over by the merge like `locked`, so no sync can undo it.
   - Picks stay as they are.
   - New domain events `MatchVoided`, `MatchUnvoided` and `PicksReopened` tell settlement (T11) to re-run the match.
5. **Reopen** unlocks the picks of a locked match whose status is `scheduled` or `postponed`. Its `scheduled_at` must be in the future, and that time becomes the new `lock_at`. Otherwise the lock ticker (T06) would lock the match again at once. An admin who needs a new time sets a schedule override first.
6. **Admin auth is pluggable** (owner's choice).
   - The API's `AppState` holds an `AdminAuth`. The production default `DenyAll` rejects every admin request with 401 until T07 plugs in session-based admin checks.
   - Dev builds use `DevHeaderAuth`, which trusts `X-Dev-User: <snowflake>` and logs a warning at startup.
   - Admin routes live under `/api/admin/matches/{id}/…` and return JSON errors: 401, 404, 409 or 422.
7. **Clock.**
   - `domain::clock` holds the `Clock` trait, `SystemClock` and `FakeClock` (frozen, moved explicitly), so every crate can take the same abstraction.
   - Reading the system time is the one impurity allowed in `domain`. That is why `domain` enables chrono's `clock` feature.
   - The API reads "now" from `AppState.clock`. T06 and T10 should do the same.
8. **Fixture source** (`sources::fixture`, feature `fixture`).
   - A JSON scenario (`sources/fixtures/scenario.json`, embedded) has fixed results and times given in minutes after an anchor. The adapter derives each match's state from the clock: scheduled, live with maps finishing in order, finished with unplayed maps marked `not_played`, or canceled at `cancel_at`.
   - Bracket slots fed by an earlier match stay TBD until it finishes.
   - Every call costs 0. The scenario is validated: known teams and stages, feeds only from earlier matches, and maps that decide the series exactly.
9. **Dev driver and endpoints** (`api::dev`).
   - `FixtureDriver` stores `{anchor, clock}` in `sync_state` (job `fixture`), so a restart keeps both. The anchor is the time of the first load, rounded down to the minute, so the dev data starts in the near future.
   - Routes: `GET /api/dev/clock` and `POST /api/dev/clock/advance {minutes}`. The second advances the clock (at most a week per call) and syncs event, then matches.
   - Jumping past a whole match produces `MatchFinished` without `MatchStarted`. Consumers must cope with that anyway, as they would after any missed poll.
10. **Dev-only parts sit behind the cargo feature `app/dev`** (owner's choice; it enables `api/dev`, which enables `sources/fixture`).
    - The feature is off by default, and production images (T16) never enable it.
    - A non-dev binary with `fixture` in `ENABLED_SOURCES` stops at startup with a clear message. A process test proves that `/api/dev/*` is a 404 in such a binary.
    - The modules are also compiled under `cfg(test)`, so plain `cargo test` covers them.

## Consequences

- T07 implements `AdminAuth` with sessions and the Manage Server / admin role check, and sets it in `app::run`.
- T08 calls `ManualService` from `/admin match result | clear-override | void | reopen`. `ManualService::overrides` lists a match's current overrides.
- T06 drives `FixtureAdapter` from the scheduler, uses `Clock` (the app's clock is the fake one in fixture mode), and can build other scenarios with `Scenario::from_json`.
- T11 re-settles on `ResultCorrected`, `MatchVoided`, `MatchUnvoided` and `PicksReopened`, and scores voided matches as 0.
- Known limit: a team correction on a match with picks (ADR 0004) is still not possible through overrides. Only status, schedule, series and maps can be overridden.
