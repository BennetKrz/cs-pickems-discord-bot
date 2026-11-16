# T06 – Sync scheduler and API request budget

- **Epic:** A – Foundation and data
- **Status:** todo
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

- [ ] With the fixture source and fake clock, a simulated busy day stays under budget: 3 followed events, 20 matches a day, 4 live at once.
- [ ] Cadence changes as a match moves from upcoming to live to finished.
- [ ] Killing the process mid-sync and restarting causes no duplicate data and no request burst.
- [ ] The README states the expected request count for a typical and a busy day.
