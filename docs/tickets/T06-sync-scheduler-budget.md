# T06 – Sync scheduler and API request budget

- **Epic:** A – Foundation and data
- **Status:** done
- **Index:** [all tickets](README.md)

**Goal:** keep the own DB fresh with as few external calls as possible.

**Depends on:** T03, T04

**Scope**

- Jobs with default cadences (all configurable):

| Data | Cadence |
| --- | --- |
| Followed event structure (stages, bracket, match list) | every 6 h and right after following |
| Matches starting in the next 48 h | every 30 min |
| Matches starting within 1 h | every 5 min, to catch reschedules before the lock |
| Live matches | every 2 min |
| Finished matches | re-check at +10 min and +2 h for corrections, then stop |
| Events not followed, finished events | never, except a manual resync |
| Event search (T09) | on demand, cached 24 h |

- Budget per source as a token bucket with hourly and per-minute caps. Default is 50% of the plan, e.g. 500/h and 30/min for PandaScore.
- Priority queue: live > imminent > corrections > structure > search. When the budget runs short, lower priorities wait and a warning is logged.
- Restart-safe: state in `sync_state`, jitter at startup, no burst of catch-up calls.
- A **lock ticker** every 30 s, independent of sync, marks matches as locked once `lock_at` has passed (rules in T10).
- Counters (requests per source per hour, last known remaining quota, last successful run per job) available to `/api/admin/status`.

**Acceptance criteria**

- [x] With the fixture source and fake clock, a simulated busy day stays under budget: 3 followed events, 20 matches a day, 4 live at once.
- [x] Cadence changes as a match moves from upcoming to live to finished.
- [x] Killing the process mid-sync and restarting causes no duplicate data and no request burst.
- [x] The README states the expected request count for a typical and a busy day.

**Outcome (2026-11-17), see [ADR 0006](../adr/0006-sync-scheduler-budget.md):**

- Owner decisions:
  - T06 adds a minimal `GET /api/admin/status`; P5 extends it.
  - `SyncHandle` plus `POST /api/admin/events/{id}/resync` for a manual resync.
- One job per kind and source, each one batched call: `live`, `imminent`, `corrections`, `upcoming`, `structure:<event>`.
  - `upcoming` (48 h) is not in the ticket's priority list. It sits between corrections and structure.
  - Overdue scheduled matches keep the 5-minute cadence for up to 6 h.
  - Canceled matches get no correction re-checks.
- Budget: two token buckets per source, plus reserves of the hourly bucket for lower priorities (5–30%). Search is skipped, not queued, when the budget is short.
- Cadences are configurable in `[sync]` / `SYNC__<KEY>`.
- The busy-day test can't use the fixture source alone, because fixtures cost 0 and hold one event. It wraps three generated fixture scenarios in a test source priced like PandaScore.
- Measured (README): a typical day uses about 305 requests, a busy day about 816. The peak is 60 per hour.

**Follow-ups:**

- **T09:** call `SyncHandle::sync_event_now` after following, and use `SyncHandle::search` for `/event search` (results are not ingested; ingest the chosen event when following).
- **T10:** the lock ticker locks at `lock_at` or on `live`/`finished`; the pick API still checks `lock_at` per request. Open pick write transactions with `store::begin_write` (`BEGIN IMMEDIATE`), as the reconciler now does, so concurrent writers wait instead of failing with "database is locked".
- **P1/P5:** there is no `MatchLocked` domain event and no `ops_events` table yet.
