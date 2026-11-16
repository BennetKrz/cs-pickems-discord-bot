# P5 – Admin status screen and alerts

- **Epic:** Proposed extras (optional)
- **Status:** proposed (optional)
- **Index:** [all tickets](README.md)

**Goal:** admins see problems before members notice wrong scores.

**Depends on:** T06, T07, T08 (P4 optional)

**Scope**

- An *Admin* tab in the Activity, visible only to admins; the API enforces it too.
- **Panels:**
  - sync jobs: last success, last error, next run;
  - API quota per source: requests per hour over the last 24 h as a chart, remaining quota from the response headers, budget cap;
  - unlinked queue (T03) with *link* and *ignore* actions;
  - active manual overrides (T05) with *clear*;
  - the last 50 warnings and errors from an `ops_events` table;
  - backups (P4), app version, uptime, database size.
- **Alerts** to a configured admin channel or Discord webhook when:
  - a sync job has failed for 30 min;
  - more than 80% of the hourly quota cap is used;
  - a live match has had no successful sync for 10 min;
  - a backup fails;
  - the unlinked queue has items older than 24 h.
- Each alert fires once per condition until it clears, followed by a *resolved* message.
- API: `GET /api/admin/status`, `GET /api/admin/ops-events`; actions reuse the T03 and T05 endpoints.

**Acceptance criteria**

- [ ] Non-admins neither see the tab nor get data (403).
- [ ] A simulated failing adapter produces exactly one alert and one resolved message.
- [ ] The quota chart matches the T06 counters.
- [ ] The status tab loads in under 1 s on the Pi.
