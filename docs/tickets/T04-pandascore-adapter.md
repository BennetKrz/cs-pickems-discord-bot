# T04 – PandaScore adapter

- **Epic:** A – Foundation and data
- **Status:** todo
- **Index:** [all tickets](README.md)

**Goal:** the first real data source.

**Depends on:** T03

**Scope**

- **Spike first (timebox 2 h)** with a real free token. Record in `docs/adr/0001-pandascore-coverage.md` which of these the free plan actually returns for CS2: match schedule and status, final series score, per-map winners, **per-map round scores**, map names, bracket links (previous matches), event tier, live updates. Set `capabilities()` from the findings.
- REST client with bearer token, pagination, and filters by tournament/serie IDs to batch requests.
- Status mapping (e.g. `not_started`, `running`, `finished`, `canceled`, `postponed`) to the canonical status.
- Read `X-Rate-Limit-Remaining` and report it to the budget (T06). On 429: exponential backoff with jitter.
- Search for events by name for T09, including tier and dates.

**Acceptance criteria**

- [ ] Spike ADR committed. If per-map round scores are missing on the free plan, tell the owner before continuing: map-score picks would then rely on a second source or on manual entry (T05).
- [ ] Conversion tests run on recorded, token-free JSON fixtures, covering Bo1, Bo3, Bo5, canceled, TBD team, and an overtime map.
- [ ] Never exceeds its declared limits in tests; 429 handling is tested.

**Notes:** PandaScore's free plan is documented as 1,000 requests per hour, and a recent changelog adds a cap of 60 per minute. Check the current numbers in the dashboard. CS endpoints may still use the `csgo` slug; verify.
