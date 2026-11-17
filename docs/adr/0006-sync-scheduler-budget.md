# 0006 – Sync scheduler and request budget

- **Status:** accepted
- **Date:** 2026-11-17
- **Ticket:** T06

## Context

T06 keeps the database fresh for followed events with as few external calls as possible. The ticket fixes the cadences, a token-bucket budget per source (default 50% of the plan: 500/h and 30/min for PandaScore), the priority order live > imminent > corrections > structure > search, restart safety via `sync_state`, a 30 s lock ticker, and counters for `/api/admin/status`. It leaves these questions open:

- Is a cadence per match or per job, and how are calls batched?
- Where does the 48 h "upcoming" job go in the priority order, which the ticket does not list?
- What does "lower priorities wait" mean for a token bucket?
- How do the fixture source, which costs 0, and its single event fit a budget test with three events?
- How do follow (T09) and a manual resync reach the scheduler, and does T06 build the status endpoint?

The owner decided the last question in the planning session: T06 adds a minimal `GET /api/admin/status` and `POST /api/admin/events/{id}/resync`.

## Decision

1. **One job per kind and source, one batched call per run.** A match belongs to at most one job, by its stored state:

   | Job (`sync_state.job`) | Matches | Call | Default cadence | Priority |
   | --- | --- | --- | --- | --- |
   | `live` | status `live` | `fetch_live`, then `fetch_matches` for IDs it no longer lists | 2 min | 1 |
   | `imminent` | `scheduled`/`postponed`, start within 1 h or overdue by at most 6 h | `fetch_matches` | 5 min | 2 |
   | `corrections` | `finished`, when end + 10 min or end + 2 h has just passed | `fetch_matches` | when due | 3 |
   | `upcoming` | `scheduled`/`postponed`, start within 48 h | `fetch_matches` | 30 min | 4 |
   | `structure:<event ID>` | each followed event that is not finished | `fetch_event` + `fetch_event_matches` | 6 h | 5 |
   | search | – | `search_events`, in-memory cache 24 h | on demand | 6 |

   - A job with no matches is skipped and costs nothing. A match changes job as its state changes, so its cadence follows upcoming → imminent → live → corrections → none by itself.
   - `upcoming` sits between corrections and structure (my choice; the ticket's list leaves it out).
   - PandaScore's running-matches route no longer lists a match that just ended, so the live job asks for those IDs again with `fetch_matches` in the same run.
   - Overdue matches keep the imminent cadence for at most 6 h (`overdue_window`). After that only the structure job sees them.
   - Correction times come from `ended_at`, falling back to `started_at`, then `scheduled_at`. Canceled matches get no correction checks.
   - Only followed events are synced, as the union over all guilds. A finished event gets no structure job. Matches of an unfollowed event are never polled.
   - Each source is asked only for matches it has an `external_refs` link for. Results always go through `Reconciler::ingest_*`, the event before its matches.
2. **Budget: two token buckets plus priority reserves** (`sync::budget`).
   - One bucket holds `per_hour` and refills `per_hour` per hour. The other holds `per_minute` and refills `per_minute` per minute. Both refill continuously.
   - A call is charged `adapter.cost(call, items)` before it is sent. A call that costs more than a bucket holds may go once that bucket is full, so nothing is blocked forever. A source without a budget entry, such as fixtures at cost 0, is unlimited.
   - Lower priorities must leave part of the hourly bucket untouched: imminent 5%, corrections 10%, upcoming 15%, structure 20%, search 30%. Live may use everything.
   - Within a tick, jobs run in priority order. When a job can't afford its call, it and every lower job of that source wait for a later tick, and one warning is logged per waiting job.
   - Search is never queued. When the budget is short, the source is skipped and named in the result, because a bot command can't wait.
   - Known limit: a full hourly bucket plus a full hour of refill allow up to 2 × `per_hour` in a rolling hour. With the default 50% budget that is the plan's own cap. The adapter's sliding-window limiter (ADR 0001) stays the hard limit.
3. **Restart safety** through `sync_state`:
   - Row (`budget`, source): `cursor` = `{hour_tokens, minute_tokens, at}`, `request_count` within the UTC clock hour in `window_started_at`, `remaining_quota` from `adapter.quota()`, and `last_run_at` = the last request. On start the buckets are restored and refilled only for the time since `at`. A restart never brings a full bucket.
   - Per job: `last_run_at` is written when the job starts, `last_success_at`/`last_error` when it ends. The corrections job keeps its checked-until time in `cursor`.
   - A job that completed waits until `last_run_at + cadence`. A job that is overdue, was interrupted (no success after the last run) or failed runs **once**, never once per missed interval.
   - During the first `startup_jitter` (60 s) after a start, a job's first run gets a random delay of up to min(cadence/2, 60 s).
   - A kill mid-job repeats at most that one job. The reconciler's per-match transactions and the unique `external_refs` prevent duplicates.
   - A failed job is retried after max(cadence, 5 min), or after the source's `retry_after` on `RateLimited`.
4. **Lock ticker** (`sync::lock`): every `lock_every` (30 s) it runs `UPDATE matches SET locked = 1 WHERE locked = 0 AND (lock_at <= now OR status IN ('live','finished'))`. This is T10's rule: lock at `lock_at` or when the match goes live, whichever comes first. It uses the app's clock, so in dev it follows the fake clock. Admin reopen (T05) moves `lock_at` into the future, so a reopened match is not re-locked. No domain event is emitted.
5. **`SyncHandle`** (cheap clone) is the only way in from outside:
   - `sync_event_now(event)` runs the structure job for every source with a link at the next tick, also for unfollowed or finished events. It wakes the loop at once. T09 calls it after following.
   - `search(query)` is budgeted and cached per source and normalized query. Results are returned, not ingested. T09 ingests the chosen event when following.
   - `status()` returns per source the budget caps, budget left, requests this hour, last request and last reported quota, and per job the last run, last success, last error and next planned run.
6. **API** (behind `AdminAuth`): `GET /api/admin/status` returns `SyncStatus`. `POST /api/admin/events/{id}/resync` answers 202, writes `audit_log` (`resync`) and calls `sync_event_now`. An unknown event is 404, and a missing scheduler 503 `sync_unavailable`. P5 extends the status endpoint.
7. **Configuration:** `[sync]` in `config.toml` and `SYNC__<KEY>` in the environment, with durations such as `30s`, `5m`, `6h`, `2d`. Keys: `structure_every`, `upcoming_window`, `upcoming_every`, `imminent_window`, `imminent_every`, `overdue_window`, `live_every`, `corrections_after` (a list), `search_cache`, `lock_every`. The internal tick (10 s), startup jitter and failure backoff are not configurable.
8. **Wiring:** `app` enables `sources/pandascore` and builds the adapter from `PANDASCORE_TOKEN`. Scheduler and lock ticker run as one task under the sync shutdown token. The `fixture` source is not scheduled: in dev, `FixtureDriver` syncs it on every clock move (T05).
9. **Write transactions start with `BEGIN IMMEDIATE`** (`store::begin_write`), in the reconciler and the manual service.
   - The lock ticker is the first writer that runs alongside the reconciler. A deferred transaction that reads, then writes, fails at once with "database is locked" when another connection wrote in between; SQLite does not apply `busy_timeout` to that upgrade.
   - The dev smoke run hit this, and `sync/tests/lock.rs` reproduced it (7 of 240 match updates failed). An immediate transaction takes the write lock first and waits up to `busy_timeout` (5 s).
10. **Budget test with fixtures.** Fixtures cost 0 and hold one event each. The tests therefore wrap several generated fixture scenarios in a test source that prices every call like PandaScore and records it with its fake time. `fastrand` (already in the workspace) provides the jitter, seeded in tests.

## Consequences

- Expected requests (from `cargo test -p sync --test scheduler -- --nocapture`):

  | Day | Followed events | Matches | Most live at once | Requests in 24 h | Peak hour | Peak minute |
  | --- | --- | --- | --- | --- | --- | --- |
  | typical | 1 | 6 | 2 | 305 | 37 | 4 |
  | busy | 3 | 20 | 4 | 816 | 60 | 13 |

  Live polling is about two thirds of the total. The budget (500/h) is never short in either case.
- T09 calls `SyncHandle::sync_event_now` after following, and `SyncHandle::search` for `/event search`.
- T10 relies on the lock ticker but still checks `lock_at` per request.
- T10 should open pick writes with `store::begin_write` too, so they wait for the sync worker instead of failing.
- P1 and P5 may want a `MatchLocked` event or an `ops_events` table; neither exists yet.
- Process tests start the app with PandaScore enabled and a dummy token. The scheduler sends nothing because no event is followed. Tests that follow an event must use the fixture or fake source.
