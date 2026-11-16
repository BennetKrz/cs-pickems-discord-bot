# T02 – Canonical domain model and database

- **Epic:** A – Foundation and data
- **Status:** todo
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
- `picks`: id, guild\_id, user\_id, match\_id, winner\_team\_id?, series\_a?, series\_b?, created\_at, updated\_at; unique (guild\_id, user\_id, match\_id). `pick_maps`: pick\_id, map\_number, rounds\_a?, rounds\_b?, overtime (bool). pick\_revisions: pick\_id, snapshot (JSON), edited\_at; one row per save, for the edit history.
- `pick_results`: pick\_id, points, breakdown (JSON), rules\_version, settled\_at.
- `guild_settings`: guild\_id, announce\_channel\_id, admin\_role\_id, scoring rules (JSON), followed events in `followed_events` (guild\_id, event\_id, followed\_by, followed\_at).
- `audit_log`: actor, action, entity, before, after, at. `sync_state`: per job and source cursors, last\_run, request counters.

**Rule on orientation:** team A/B of a match is fixed when the match is first created. Every score (source data and picks) is stored relative to it. If a source reports the teams swapped, the reconciler (T03) flips the data, never the match.

**Acceptance criteria**

- [ ] Migrations run on an empty DB and are idempotent on restart.
- [ ] SQLite runs with `journal_mode=WAL`, `foreign_keys=ON`, `busy_timeout=5000`.
- [ ] Constraints are enforced and tested: one pick per user and match, valid format and status values, map number within the format.
- [ ] Repository integration tests run against a temp DB.
