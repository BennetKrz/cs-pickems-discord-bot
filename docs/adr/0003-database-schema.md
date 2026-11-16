# 0003 – Database schema conventions

- **Status:** accepted
- **Date:** 2026-11-16
- **Ticket:** T02

## Context

T02 creates the schema that every later ticket reads from. The ticket names the tables and columns but leaves open how values are typed, which invariants the database itself enforces, and how the store is queried. Facts that shaped the choices:

- SQLite `CHECK` constraints can't read other tables, and changing a `CHECK` later needs a table rebuild.
- sqlx's default chrono encoding writes `2026-11-20T10:00:00+00:00` with a variable-length fraction. That is not fixed width, so comparing timestamps as strings in SQL is fragile.
- sqlx's compile-time `query!` macros need a live `DATABASE_URL` or a committed `.sqlx` offline cache in every build, including CI (T17) and the Docker image (T16).
- sqlx 0.9 accepts only `&'static str` SQL unless it is wrapped in `AssertSqlSafe`.
- T02 lists `pick_maps` without a winner column, but T10 requires a winner for every map pick, including winner-only and overtime picks.

## Decision

1. **Types**
   - Every table is `STRICT`.
   - Internal IDs are `INTEGER PRIMARY KEY`.
   - Discord snowflakes are `TEXT` of digits, checked with `NOT GLOB '*[^0-9]*'`, matching the `String` IDs in config.
   - Booleans are `INTEGER CHECK (x IN (0, 1))`.
   - JSON columns are `TEXT CHECK (json_valid(x))`.
2. **Timestamps**
   - Stored as UTC `TEXT` in exactly one form, `YYYY-MM-DDTHH:MM:SS.sssZ` (millisecond precision, truncated), and every timestamp column checks it with `GLOB`. String order then equals time order, so `lock_at <= ?` is correct.
   - Writes go through the `store::Ts` bind wrapper. Reads use sqlx's chrono decoder.
   - Repositories never read the clock; callers pass `now`, so the fake clock (T05) works.
3. **Enums**
   - Every enum column has a `CHECK (x IN (...))` with the string forms from `domain::model`.
   - A test inserts every `ALL` value of each domain enum, so the two can't drift apart.
   - Values the ticket left open:
     - `events.status`: `upcoming | running | finished | canceled`
     - `maps.status`: `upcoming | live | finished | not_played`
     - `EntityType`: `team | event | stage | match`
4. **Cross-table rules are triggers** (`RAISE(ABORT, …)` with a readable message):
   - Map and map-pick numbers must be ≤ the match format (1, 3 or 5).
   - Every map, pick and map-pick winner must be one of the match's two teams. Same-row checks use `IS`, so a TBD (NULL) team never lets a winner through.
   - A match's format can't shrink below its existing maps or map picks.
   - A team named as a winner can't be removed from the match.

   Format-specific series scores (for example 2:1 only in a Bo3) stay in T10's domain rules; the schema only checks the range 0–3.
5. **Small schema changes from the ticket text**
   - `pick_maps.winner_team_id NOT NULL` was added; the owner confirmed this.
   - `stages.order` is named `position`, because `order` is an SQL keyword.
   - `feeds_a_from` / `feeds_b_from` are pairs of columns (`*_match_id`, `*_outcome`) that must both be set or both be NULL.
   - `followed_events` is its own table, keyed by (guild, event).
   - `external_refs` and `source_records` are keyed by (source, entity_type, external_id).
6. **`source_records` keeps only the latest payload** per external entity.
   - Replay (T03) only needs the latest; history would grow without bound on the Pi.
   - The payload is rewritten only when `payload_hash` changes.
   - History can come later as a new table.
7. **Pick revisions cascade.** Deleting a pick (T10: removing every part) also deletes its map parts, revisions and result, as the ticket's foreign keys imply.
8. **Runtime-checked queries**
   - Queries use `sqlx::query` with `concat!`-built static SQL. Rows map to domain types by hand, through `try_get` plus a shared enum parser.
   - There are no `query!` macros, so builds need no database and no `.sqlx` cache.
   - Integration tests against a temp database replace the compile-time check.
9. **Repository shape**
   - Free functions per area.
   - Single statements take `impl SqliteExecutor`, which can be a pool, a connection or a transaction.
   - Multi-statement operations (`picks::save`, `refs::upsert_record`) take `impl Acquire` and open a nested transaction, so callers like the reconciler (T03) can compose them inside their own transaction.
10. **Migrations**
    - Embedded with `sqlx::migrate!()` and applied by `store::open` at every start. Applied versions are recorded in `_sqlx_migrations`, so a restart is a no-op.
    - Migrations are forward-only and must stay backward-compatible (Watchtower deploys unattended).
    - A released migration file is never edited: sqlx checks its checksum and refuses to start.

## Consequences

- Adding an enum value (for example a new `EntityType` or match status) needs a migration that rebuilds the table, following SQLite's 12-step `ALTER TABLE` procedure, plus the domain change.
- A wrong column name or type in SQL surfaces in tests, not at compile time; keep repository test coverage complete.
- T03's reconciler must not swap team A/B of an existing match. The triggers block replacing a team that existing picks or map results name as winner, so such corrections must handle the affected picks first.
- Follow-ups that later tickets will need and that are not in T02's scope:
  - T03: field provenance (which source set each value) and the unlinked queue.
  - T05: void and reopen markers on matches.
  - T11: provisional or final state in `pick_results`, and a history of scoring-rule versions.
