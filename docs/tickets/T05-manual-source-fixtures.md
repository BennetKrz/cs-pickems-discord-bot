# T05 – Manual source, admin overrides and dev fixtures

- **Epic:** A – Foundation and data
- **Status:** todo
- **Index:** [all tickets](README.md)

**Goal:** a fallback when sources are wrong or missing, and realistic test data without API quota.

**Depends on:** T03

**Scope**

- `manual` source as a service plus admin API endpoints: set or clear the status, schedule, series score and map scores of a match. Overrides win over every source and survive the next sync. Clearing an override falls back to the source value.
- Admin actions on a match: **void** (no points for anyone, picks kept for history) and **reopen picks** (only before the match is live, e.g. after a long postponement).
- Every manual change writes `audit_log` with the Discord user.
- `fixture` source for development (enabled by `ENABLED_SOURCES=fixture`): loads a scenario file with an event that has a Swiss stage and a double-elimination playoff, Bo1/Bo3/Bo5 matches, a TBD slot, a canceled match and an overtime map. A dev-only endpoint advances a fake clock so matches go live and finish, emitting the normal domain events.

**Acceptance criteria**

- [ ] A manual score beats the PandaScore value and persists after a sync; clearing restores the source value.
- [ ] Void and reopen are audited and trigger re-settlement (T11).
- [ ] The fixture scenario loads, and advancing the clock drives a full match lifecycle.
- [ ] Dev-only endpoints are compiled out or disabled in production builds.

**Notes:** the Discord commands for these actions belong to T08.
