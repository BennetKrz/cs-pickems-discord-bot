# 0004 – Source adapters and reconciliation

- **Status:** accepted
- **Date:** 2026-11-16
- **Ticket:** T03

## Context

T03 lets any number of sources feed one canonical model. The ticket fixes the shape: an adapter trait, a reconciler with `source_records` and `external_refs`, field precedence with `manual` first, identity rules, an unlinked queue, domain events and a replay command. It leaves open the questions below. The owner decided the first two in the T03 planning session.

- What does `source_records.payload` hold, given that replay and precedence merging both read it back?
- Who may create canonical entities when a second source reports something unknown?
- How are fields grouped for precedence, and what happens to sources a group does not list?
- How exactly are names compared, and when does each domain event fire?
- `async fn` in traits is not `dyn`-compatible, but the scheduler (T06) needs a registry of adapters.

## Decision

1. **Payload = `{"v": 1, "dto": …, "raw": …}`** (owner's choice).
   - `dto` is the source-neutral DTO from `sources::dto`. `raw` is the source's own JSON (`null` for embedded teams and for manual entries).
   - Merging and replay read only `dto`, so they need no adapter code and no tokens. `raw` is kept for debugging and for a possible re-decode later.
   - `payload_hash` is SHA-256 over the serialized payload. The serialization is deterministic: struct fields in declaration order, `serde_json` maps sorted.
   - DTO changes must stay backward-compatible (add optional fields only) or bump `DTO_VERSION`.
   - Teams embedded in a match are also stored as their own `team` records. Stages live inside their event's record.
2. **Only the primary source creates entities** (owner's choice).
   - The primary is `PRIMARY_SOURCE` / `primary_source`, by default the first non-`manual` entry of `ENABLED_SOURCES`.
   - It creates events, stages, teams and matches, with no identity matching, so two of its own teams with similar names never merge.
   - Every other source links through:
     1. an existing `external_refs` link (e.g. set by an admin);
     2. for teams, an exact normalized name or acronym;
     3. for matches, the same canonical event, the same two teams in either order, and `scheduled_at` within ±2 h (inclusive);
     4. for stages, an exact normalized name within the linked event.

     Events of non-primary sources have no automatic rule; they need a manual link.
   - Everything else goes to `unlinked_refs` with reason `no_match`, `ambiguous` or `dependency` (its event or a team is not linked). Every ingest of a source retries that source's queue, so a secondary source that is ahead of the primary resolves by itself.
   - Ambiguity is decided on **all** candidates. If the single candidate is already linked to another ID of the same source, the entity is queued as `no_match`. So linking one of two look-alikes ("Liquid" / "Team Liquid") never auto-links the other.
   - Admins resolve the queue with `Reconciler::link`, which can also move a wrong link, or `create_from_unlinked`. Both write `audit_log`. A match can only be created from the queue once its event and teams are linked.
   - Because nothing is created on a mismatch, linking never has to merge two canonical entities.
3. **`manual` is a source without an adapter.**
   - It is always first in every group. It never creates entities and is never queued.
   - Its records apply only through links, which T05 sets with `external_id` = the canonical ID. T05 calls `Reconciler::refresh_match` after a change.
   - A `manual` DTO has no teams, so it is never flipped: manual values are in canonical orientation.
4. **Field groups** (config keys and `field_sources.field`):

   | Group | Fields |
   | --- | --- |
   | `event` | event name, tier, dates, status |
   | `stage` | stage name, kind, position |
   | `team` | team name, acronym, logo, country |
   | `teams` | the two teams of a match |
   | `format` | Bo1/Bo3/Bo5 |
   | `schedule` | `scheduled_at` |
   | `status` | status, `started_at`, `ended_at` |
   | `series` | maps won, series winner |
   | `bracket` | stage, side, round, slot, feeds |
   | `map_names.N` | name of map N |
   | `map_rounds.N` | status, rounds and winner of map N |

   - Per group, the first source in order **that has a value** wins.
   - For `event`, `stage` and `team`, the top source gives the name, and gaps in the optional fields are filled from lower sources.
   - Enabled sources a group does not list follow the listed ones, in `ENABLED_SOURCES` order. Listed sources that are not enabled are dropped. Sources that are not enabled are ignored, even if their old records are still stored.
   - A group no source has a value for keeps its current canonical value and provenance.
   - `lock_at` follows `scheduled_at` while the match is unlocked. `locked` is never touched (T06/T10).
5. **Orientation.**
   - Each view is compared with the existing canonical teams (or, for a new match, the top view's). It is flipped when more of its teams match crossed than straight; a view that shares no team is not flipped.
   - Flipping swaps the score, the map rounds, the map and series winners, and the bracket feeds.
   - A TBD slot is filled without moving the known team. Teams of an existing match are never swapped.
   - A score that is impossible for the format, or map numbers beyond it, are ignored. A missing series winner is derived from a decisive score.
   - A new match whose sources give no format is stored as Bo3.
6. **Domain events** (`domain::DomainEvent`, published on a `tokio::sync::broadcast` bus with capacity 1024, **after** the commit):

   | Event | When |
   | --- | --- |
   | `MatchCreated` | A match is inserted. This is its only event, even if it arrives finished. |
   | `MatchRescheduled` | `scheduled_at` changed. |
   | `MatchStarted` / `MatchFinished` / `MatchCanceled` | The status changed to `live` / `finished` / `canceled`. |
   | `MapUpdated` | Any value of a map changed. |
   | `ResultCorrected` | The match was finished before and after, and the series or a map result changed. |

   `postponed` has no event of its own; a new time emits `MatchRescheduled`. Re-ingesting an identical payload writes nothing canonical and emits nothing. Only `source_records.fetched_at` moves, as in T02.
7. **Transactions.** One transaction per match (with its teams), per event, and per admin action. A failing match is logged, counted in `Report::failed` and skipped; the rest of the batch continues.
8. **Replay** (`app reconcile --replay`):
   - Re-reconciles every stored record. It processes the primary source first, then the others in precedence order. Per source, it handles events, then teams, then matches; a second pass resolves bracket feeds.
   - It works on top of the existing data, because wiping it would break picks' foreign keys. On an empty database it rebuilds everything, though internal IDs may differ. Links an admin set must still exist for the linked sources to count.
   - It makes no API calls. Its events go nowhere, because no consumer runs in that process. It exits non-zero if a record failed.
9. **Libraries.**
   - `async-trait` makes `SourceAdapter` usable as `dyn SourceAdapter`.
   - `sha2` computes the payload hash.
   - `clap` (derive) parses the subcommands. Without a subcommand, the app runs as before.
   - Rust 2024 async closures implement the reconciler's transaction helper. A test checks that the reconciler's futures are `Send`.
10. **Adapters behind cargo features.** Each adapter is one module behind one feature of the `sources` crate. T03 adds `fake`, an in-memory adapter for tests of other crates; T04 adds `pandascore`, T05 `manual` and `fixture`. A new source is one module, one `SourceId` variant and one entry in `ENABLED_SOURCES`.

## Consequences

- T04 converts PandaScore JSON into `SrcEvent`/`SrcMatch` and passes the original object as `raw`. Rate limits come from `limits()` and `cost()`; the budget stays in config.
- T05 writes `manual` records and links, and calls `refresh_match`. Clearing an override means removing its field from the manual DTO, so the next source in order applies again.
- T06 owns the adapter registry and calls `ingest_event`/`ingest_matches`. It should ingest a followed event before its matches; matches of an unknown event are queued and retried anyway.
- T08's `/admin link` lists `store::unlinked::list` and calls `link` or `create_from_unlinked`.
- T10/T11/T08 subscribe to `EventBus`. A lagging subscriber gets `RecvError::Lagged` and must re-read state from the database.
- Known limits:
  - A team correction on a match whose picks or map results name the old team is rejected by the T02 triggers. That match fails and stays as it was until an admin resolves it (T05).
  - A format change to fewer maps than already exist is rejected the same way.
  - A map that no source reports any more is kept, not deleted.
