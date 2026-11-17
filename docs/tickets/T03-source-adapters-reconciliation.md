# T03 – Data-source adapter framework and reconciliation

- **Epic:** A – Foundation and data
- **Status:** done
- **Index:** [all tickets](README.md)

**Goal:** any number of sources feed one canonical model, and the app only ever sees that model.

**Depends on:** T02

**Scope**

- Trait `SourceAdapter` (async): `id()`, `capabilities()` (which of: schedule, results, map\_scores, map\_names, brackets, live, search), `search_events(query)`, `fetch_event(ext_id)` (with stages and bracket links), `fetch_event_matches(ext_id)`, `fetch_matches(ext_ids)`, `fetch_live(ext_ids)`. Each returns source-neutral DTOs (`SrcEvent`, `SrcMatch`, `SrcMap`, `SrcTeam`) carrying external IDs.
- Adapters only fetch and convert. They never touch the DB. Each one declares its rate limits and cost per call for the budget (T06).
- **Reconciler:** stores the raw payload in `source_records`, resolves entities through `external_refs`, and creates missing ones. Cross-source identity for an entity a second source reports: (1) an existing manual link, (2) for teams an exact normalized name or acronym, (3) for matches the same event, same two teams and scheduled time within ±2 h. Anything else lands in an *unlinked* queue that admins resolve (T08 command).
- **Field precedence** comes from config per field group, e.g. `schedule: [manual, pandascore]`, `map_rounds: [manual, <source2>, pandascore]`. `manual` always comes first. Store which source set each current value.
- **Domain events** on an internal broadcast channel, emitted only on real change: `MatchCreated`, `MatchRescheduled`, `MatchStarted`, `MapUpdated`, `MatchFinished`, `MatchCanceled`, `ResultCorrected`. Consumers: lock (T10), settlement (T11), announcements (T08).
- CLI subcommand `app reconcile --replay` rebuilds canonical data from `source_records` without API calls.

**Acceptance criteria**

- [x] Two fake adapters that disagree resolve per the configured precedence; a test proves it.
- [x] A source with swapped team order is stored correctly flipped.
- [x] Reconciling the same payload twice changes nothing and emits no events.
- [x] Unlinked entities show up in the queue and can be linked; linking merges without duplicates.

**Notes:** keep adapters in separate modules behind cargo features, so a new source is one module plus one config entry.

Implementation decisions are in [ADR 0004](../adr/0004-source-adapters-reconciliation.md). The owner chose the `{dto, raw}` payload and that only the primary source creates entities. Follow-ups for later tickets:

- **T04:** add the `pandascore` feature and module, implement `SourceAdapter`, pass the original JSON as `raw`, and add `SourceId::Pandascore` to the adapter registry.
- **T05:** the `manual` service writes `manual` records and links (`external_id` = canonical ID) and calls `Reconciler::refresh_match`. Clearing an override removes the field from the manual DTO. A team correction on a match whose picks name the old team is blocked by the T02 triggers and needs an admin path.
- **T06:** own the adapter registry, call `ingest_event` before `ingest_matches`, and start the `EventBus` consumers. `sync::run` already receives the bus.
- **T08:** `/admin link` uses `store::unlinked::list`, `Reconciler::link` and `Reconciler::create_from_unlinked`.
