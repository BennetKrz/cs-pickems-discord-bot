# 0001 – PandaScore free-plan coverage

- **Status:** accepted
- **Date:** 2026-11-17
- **Ticket:** T04

## Context

T04 starts with a spike: which CS2 data does PandaScore's free plan actually return? On 2026-11-17 about 50 requests were made with a real free token against `https://api.pandascore.co`. The official docs were checked the same day.

| Data | Free plan | Where |
| --- | --- | --- |
| Match schedule and status | yes | `/csgo/matches` (also `/upcoming`, `/running`, `/past`). Statuses: `not_started`, `running`, `finished`, `canceled`, `postponed`. Also `original_scheduled_at`, `rescheduled`. |
| Final series score | yes | `results[] {team_id, score}`. The order often differs from `opponents[]`. |
| Per-map winners | yes | `games[] {position, status, winner.id}`. Unplayed games of a decided series are left out (a Bo5 won 3:0 lists 3 games). |
| **Per-map round scores** | **no** | `/csgo/games/{id}`, `/csgo/games/{id}/rounds` and `/csgo/matches/{id}/games` return **403** "Access Denied" |
| **Map names** | **no** | same 403. `/csgo/maps` lists the map pool, but a game never names its map. |
| Bracket links | yes | `/tournaments/{id}/brackets` → `previous_matches[] {type: winner\|loser, match_id}`. They are not on `/csgo/matches`. The order does not say which slot is fed. |
| Event tier | yes | `tier` (`s`, `a`, `b`, `c`, `d`) on each PandaScore *tournament*, not on the serie |
| Live updates | REST only | A running match lists finished maps' winners and the current map as `running`. The websocket in `live.url` is a paid real-time product and was not tested. |
| Event search | yes | `/csgo/series?search[slug]=blast-rivals`. Serie slugs contain the league name, e.g. `cs-go-blast-rivals-fall-2026`. Results embed tournaments with tier and dates. |
| Batching | yes | `filter[id]=a,b,c`, `filter[serie_id]`, `filter[tournament_id]`, `per_page` up to 100, `Link: <…>; rel="next"`, `X-Total` |

- **Route slug:** CS2 still uses `csgo` (`/cs2/...` answers 404). The videogame slug is `cs-go`; `videogame_title` says "Counter-Strike 2".
- **Rate limits:**
  - 1,000 requests per hour, reported as `X-Rate-Limit-Remaining` and `X-Rate-Limit-Used`.
  - Free tokens also have a burst cap of 60 per minute (changelog 2.63.0); no header reports it.
  - The docs do not describe the 429 response, so a `Retry-After` header is not guaranteed.
- **Walkovers** come as `status: canceled`, `forfeit: true`, a `winner_id`, 0:0 and no played games.
- The token's hourly counter was not at zero when the spike started, so the same key may be in use elsewhere.

## Decision

1. **Continue with PandaScore as the primary source, without map scores** (owner's choice).
   - `capabilities()` = `Schedule`, `Results`, `Brackets`, `Live` (REST polling), `Search`.
   - No `MapScores` and no `MapNames`: the adapter never sets `SrcMap.rounds` or `SrcMap.name`.
   - Round scores and map names come from manual entry (T05), and the pending-then-void rule (T11) covers maps nobody entered. A second source can be added later without changes here.
2. **Mapping.**
   - A PandaScore **serie** is our event. Its name is "league name + `full_name`", e.g. "BLAST Rivals Fall 2026". Its **tournaments** are our stages, positioned by `begin_at`, then ID.
   - Event tier is the best tier among the tournaments; it is `None` if none is known, so a lower source can fill it.
   - Event status is `finished` when the serie has a winner and `None` otherwise. Nothing else in the data says, and Austin LAN 2026 was over with no winner set.
   - Stage kind: a name containing "swiss" → `swiss`, "group" → `groups`, anything else `None`. PandaScore doesn't say single or double elimination.
   - Teams: `opponents[0]` is A, `[1]` is B. A missing opponent is TBD. An empty `location` is no country.
   - Format: `match_type: best_of` with 1, 3 or 5 games; anything else `None`.
   - Status: `not_started` → `scheduled`, `running` → `live`, `finished` → `finished`, `canceled` → `canceled`, `postponed` → `postponed`. An unknown status → `None` plus a warning.
   - Score: only for live or finished matches with both teams known, matched by `team_id`.
   - Winner: only for finished matches that are not a `draw`.
   - Maps:
     - `finished` → finished with a winner; `running` → live; `not_started` → upcoming.
     - In a finished or canceled match, every map up to the format's maximum that was not played is `not_played`, including the ones PandaScore leaves out.
   - Bracket:
     - Only matches of a tournament with `has_bracket` get bracket data.
     - A feed goes to the slot of the team it produced, when the earlier match is decided. Otherwise the listed order is kept (first → A).
     - Side comes from the match name ("Upper Bracket", "Lower Bracket", "Grand Final"). Round and slot stay `None`; T13 can derive them from the feeds.
3. **Walkovers are canceled matches with no winner and no score** (owner's choice), so T11 voids picks on them.
4. **Calls** (`ext_id`s are PandaScore's numeric IDs as strings):

   | Method | Requests | Estimated `cost()` |
   | --- | --- | --- |
   | `search_events` | `/csgo/series?search[slug]=<slugified query>&sort=-begin_at&per_page=25` | 1 |
   | `fetch_event` | `/series/{id}`; a non-CS serie → `NotFound` | 1 |
   | `fetch_event_matches` | `/csgo/matches?filter[serie_id]` (all pages) + `/tournaments/{id}/brackets` per bracket stage | 3 |
   | `fetch_matches` | `/csgo/matches?filter[id]=…`, 100 IDs per request | ⌈n/100⌉ |
   | `fetch_live` | `/csgo/matches/running?filter[id]=…` | ⌈n/100⌉ |

   `raw` is the PandaScore object as received. For bracket matches, `previous_matches` from the brackets route is merged into it.
5. **Rate limiting inside the adapter.**
   - The plan's limits (default 60/min and 1,000/h) are enforced with sliding windows before every request, including retries and extra pages. If a window is full, the call fails with `RateLimited { retry_after }` and nothing is sent. T06's budget (50% of the plan) sits on top of this.
   - When `X-Rate-Limit-Remaining` reaches 0, the adapter holds back for 60 s, then lets one request through as a probe.
   - The last reported quota is available as `SourceAdapter::quota()`, a new default trait method, for T06.
   - On 429: up to 3 retries with exponential backoff (1 s base, doubling, 30 s cap) and equal jitter. A numeric `Retry-After` is honored up to the cap; above the cap, the call fails at once with that `retry_after`.
6. **Libraries.**
   - `reqwest` 0.13 without default features, with `rustls` and `http2`.
   - `fastrand` for jitter.
   - Both are optional dependencies behind the `pandascore` feature. As with `fixture`, the module is also compiled under `cfg(test)`, so plain `cargo test` covers it.
   - Tests use a local axum server and recorded fixtures; no test calls the real API except one `#[ignore]`d smoke test.
7. **Fixtures** (`backend/crates/sources/fixtures/pandascore/`).
   - The responses are real, recorded on 2026-11-17, trimmed to single objects, with `streams_list` emptied in the list files. They contain no token or account data.
   - `match_running.json` is the one edited file: no CS match was running during the spike, so it is the recorded Bo3 changed to "running after map 1".
   - The ticket's "overtime map" case cannot exist in free-plan data, because there are no rounds. Instead (owner's choice), tests prove that no map ever gets rounds or a name, and cover postponed, draw, unknown-status, walkover and TBD cases.

## Consequences

- Exact map-score picks on PandaScore-only data stay pending until an admin enters the map (T05), or they are voided (T11). The UI should make that visible (T12).
- T06 builds `PandascoreAdapter::new(PandascoreConfig::new(token))` from `PANDASCORE_TOKEN` and enables the `sources/pandascore` feature in `app`. It reads `quota()` after calls and treats `RateLimited` as "try later". Matches of a followed event should be refreshed with `fetch_event_matches` now and then, because `fetch_matches` carries no bracket links.
- `reqwest`'s default TLS backend is rustls with `aws-lc-rs`, which compiles C code. The arm64 cross-build (T16/T17) needs a C cross toolchain (e.g. `cargo zigbuild` or `cross`).
- If a paid plan is ever used, the game detail route could fill `rounds` and `name`. That would be a change in `convert.rs` plus the two capabilities.

## Sources

Checked 17 Nov 2026:

- [PandaScore rate and connection limits](https://developers.pandascore.co/docs/rate-and-connections-limits)
- [PandaScore changelog: updated free tier rate limits (60/min burst)](https://developers.pandascore.co/changelog/updated-free-tier-rate-limits)
- Spike requests against `api.pandascore.co` (responses recorded as fixtures)
