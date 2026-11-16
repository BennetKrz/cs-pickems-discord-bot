# T02 – Canonical domain model and database

- **Epic:** A – Foundation and data
- **Status:** in review
- **Index:** [all tickets](README.md)

**Goal:** one schema that is the single source of truth for all reads.

**Depends on:** T01

**Scope (tables, sqlx migrations, repositories)**

- `teams`: id, name, acronym, logo\_url, country.
- `events`: id, name, tier (`s`/`a`/`b`/`c`/`d`/unknown), begin\_at, end\_at, status.
- `stages`: id, event\_id, name, kind (`swiss`, `groups`, `single_elim`, `double_elim`, `other`), order.
- `matches`: id, event\_id, stage\_id, team\_a\_id and team\_b\_id (nullable for TBD), format (`bo1`/`bo3`/`bo5`), status (`scheduled`, `live`, `finished`, `canceled`, `postponed`), scheduled\_at, started\_at, ended\_at, lock\_at, locked (bool), maps\_won\_a, maps\_won\_b, winner\_team\_id, plus bracket fields: bracket\_side (`upper`/`lower`/`final`/null), round\_index, slot\_index, feeds\_a\_from and feeds\_b\_from (match id + `winner`/`loser`).
- `maps`: match\_id, number (1–5), map\_name (nullable until veto), status, rounds\_a, rounds\_b, winner\_team\_id.
- `external_refs`: entity\_type, entity\_id, source, external\_id; unique on (source, entity\_type, external\_id).
- `source_records`: source, entity\_type, external\_id, payload (JSON), payload\_hash, fetched\_at. Raw data for replay without API calls.
- `users`: discord\_user\_id (PK), username, global\_name, avatar\_hash, updated\_at. `guild_members`: guild\_id, user\_id, nick, last\_seen\_at.
- `picks`: id, guild\_id, user\_id, match\_id, winner\_team\_id?, series\_a?, series\_b?, created\_at, updated\_at; unique (guild\_id, user\_id, match\_id). `pick_maps`: pick\_id, map\_number, winner\_team\_id, rounds\_a?, rounds\_b?, overtime (bool). pick\_revisions: pick\_id, snapshot (JSON), edited\_at; one row per save, for the edit history.
- `pick_results`: pick\_id, points, breakdown (JSON), rules\_version, settled\_at.
- `guild_settings`: guild\_id, announce\_channel\_id, admin\_role\_id, scoring rules (JSON), followed events in `followed_events` (guild\_id, event\_id, followed\_by, followed\_at).
- `audit_log`: actor, action, entity, before, after, at. `sync_state`: per job and source cursors, last\_run, request counters.

**Rule on orientation:** team A/B of a match is fixed when the match is first created. Every score (source data and picks) is stored relative to it. If a source reports the teams swapped, the reconciler (T03) flips the data, never the match.

**Acceptance criteria**

- [x] Migrations run on an empty DB and are idempotent on restart.
- [x] SQLite runs with `journal_mode=WAL`, `foreign_keys=ON`, `busy_timeout=5000`.
- [x] Constraints are enforced and tested: one pick per user and match, valid format and status values, map number within the format.
- [x] Repository integration tests run against a temp DB.

**Notes**

- Decisions are in [ADR 0003](../adr/0003-database-schema.md):
  - STRICT tables, INTEGER IDs, TEXT snowflakes;
  - one fixed-width UTC timestamp format, checked in the schema;
  - CHECKs for enums, triggers for cross-table rules;
  - runtime-checked queries instead of `query!` macros;
  - latest-only `source_records`;
  - cascading pick deletion.
- Changes from the scope text:
  - `pick_maps.winner_team_id` was added, because T10 requires a winner for every map pick; the owner confirmed this.
  - `stages.order` is named `position`.
  - The bracket feeds are column pairs (`feeds_x_from_match_id`, `feeds_x_from_outcome`).
- Values the scope left open:
  - `events.status`: `upcoming | running | finished | canceled`
  - `maps.status`: `upcoming | live | finished | not_played`
- Migrations run in `store::open` at every start. The schema is in `backend/crates/store/migrations/0001_initial.sql` and the domain types are in `backend/crates/domain/src/model.rs`.
- Tests are in `backend/crates/store/tests/`:
  - `migrations.rs`: empty database, idempotent restart, pragmas on every pooled connection, timestamp format;
  - `constraints.rs`: one pick per user and match, enum values, map number vs format, winners vs teams, row checks, foreign keys, external ID uniqueness;
  - `repos.rs`: round trips for every repository.
- Follow-ups:
  - T03: record which source set each field value, and add the unlinked queue. Never swap team A/B; triggers block replacing a team that existing picks name as winner.
  - T05: void and reopen markers on matches.
  - T11: provisional or final state in `pick_results`, and a history of scoring-rule versions, so "rules active at lock time" can be looked up.
