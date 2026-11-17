# T04 – PandaScore adapter

- **Epic:** A – Foundation and data
- **Status:** in review
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

- [x] Spike ADR committed. If per-map round scores are missing on the free plan, tell the owner before continuing: map-score picks would then rely on a second source or on manual entry (T05).
- [x] Conversion tests run on recorded, token-free JSON fixtures, covering Bo1, Bo3, Bo5, canceled, TBD team, and an overtime map. (The overtime case was replaced with the owner's agreement; see the outcome below.)
- [x] Never exceeds its declared limits in tests; 429 handling is tested.

**Notes:** PandaScore's free plan is documented as 1,000 requests per hour, and a recent changelog adds a cap of 60 per minute. Check the current numbers in the dashboard. CS endpoints may still use the `csgo` slug; verify.

**Outcome (2026-11-17), see [ADR 0001](../adr/0001-pandascore-coverage.md):**

- Spike:
  - The free plan has no per-map round scores and no map names; the game routes answer 403.
  - The owner was told and decided to continue: map scores come from manual entry (T05), with pending-then-void (T11).
  - The route slug is still `csgo`. The limits of 1,000/h and 60/min are confirmed.
- Overtime fixture:
  - It can't exist without round data.
  - With the owner's agreement, it was replaced by tests proving that no map gets rounds or a name, plus postponed, draw, walkover and unknown-status cases.
- `match_running.json` is the only edited fixture, because no match was running during the spike.
- Walkovers (`canceled` + `forfeit` + `winner_id`) are reported as canceled with no winner (owner's choice).
- New trait method `SourceAdapter::quota()` gives T06 the last `X-Rate-Limit-Remaining`.
- Follow-ups:
  - T06 enables `sources/pandascore` in `app` and builds the adapter from `PANDASCORE_TOKEN`.
  - T16/T17 need a C cross toolchain for `aws-lc-rs` (rustls).
  - The websocket live feed was not tested; it is a paid product.
  - Manual smoke test: `PANDASCORE_TOKEN=… cargo test -p sources --features pandascore -- --ignored live_smoke` (about 3 requests).
