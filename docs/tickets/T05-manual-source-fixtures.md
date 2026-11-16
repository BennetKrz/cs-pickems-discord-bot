# T05 – Manual source, admin overrides and dev fixtures

- **Epic:** A – Foundation and data
- **Status:** in review
- **Index:** [all tickets](README.md)

**Goal:** a fallback when sources are wrong or missing, and realistic test data without API quota.

**Depends on:** T03

**Scope**

- `manual` source as a service plus admin API endpoints: set or clear the status, schedule, series score and map scores of a match. Overrides win over every source and survive the next sync. Clearing an override falls back to the source value.
- Admin actions on a match: **void** (no points for anyone, picks kept for history) and **reopen picks** (only before the match is live, e.g. after a long postponement).
- Every manual change writes `audit_log` with the Discord user.
- `fixture` source for development (enabled by `ENABLED_SOURCES=fixture`): loads a scenario file with an event that has a Swiss stage and a double-elimination playoff, Bo1/Bo3/Bo5 matches, a TBD slot, a canceled match and an overtime map. A dev-only endpoint advances a fake clock so matches go live and finish, emitting the normal domain events.

**Acceptance criteria**

- [x] A manual score beats the PandaScore value and persists after a sync; clearing restores the source value.
- [x] Void and reopen are audited and trigger re-settlement (T11).
- [x] The fixture scenario loads, and advancing the clock drives a full match lifecycle.
- [x] Dev-only endpoints are compiled out or disabled in production builds.

**Notes:** the Discord commands for these actions belong to T08.

Implementation decisions are in [ADR 0005](../adr/0005-manual-overrides-and-fixtures.md). The owner chose:
- pluggable admin auth that denies everything in production until T07;
- an unvoid action;
- resetting a cleared score to neutral when no source has one;
- a cargo feature `dev` for the dev-only parts.

- **AC 2:** void, unvoid and reopen emit `MatchVoided`, `MatchUnvoided` and `PicksReopened` on the event bus. Manual score changes on finished matches emit `ResultCorrected`. The settlement that consumes these events is built in T11.
- **AC 4:** everything dev-only sits behind `app/dev`. A process test checks the non-dev binary: `/api/dev/*` is a 404 and `ENABLED_SOURCES=fixture` refuses to start. Dev wiring checks: `cargo clippy --all-targets --features app/dev -- -D warnings && cargo test --features app/dev`.

Follow-ups for later tickets:

- **T06:** drive `FixtureAdapter` from the scheduler. Use `domain::Clock`; in fixture mode the app clock is the fake one. Other scenarios can be built with `Scenario::from_json`.
- **T07:** implement `api::admin::AdminAuth` with sessions and the admin rule, and set it in `app::run` in place of `DenyAll`.
- **T08:** `/admin match …` calls `sync::ManualService`. `overrides()` lists the current overrides.
- **T11:** re-settle on the three new events and on `ResultCorrected`, and score voided matches as 0. A clock jump can skip `MatchStarted`, as a missed poll can.
- Still open (ADR 0004 known limit): correcting a team on a match whose picks name the old team.
