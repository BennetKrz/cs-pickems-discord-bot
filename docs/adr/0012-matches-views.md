# 0012 – Match list, match detail and the server clock

- **Status:** accepted
- **Date:** 2026-11-22
- **Ticket:** T12

## Context

T12 adds the *Matches* tab (Upcoming, Live, Results) and the match detail with the pick form and *Everyone's picks*. The ticket names the segments, the windows (14 days ahead, 30 days back) and the two routes, but leaves open:

- **"Full" pick.** Rows show *no pick / partial / full*, but a pick has three optional parts and map scores are optional too.
- **"Needs my pick".** Only a missing pick, or a partial one too?
- **Which matches the list shows.** Followed events only, or also matches a member picked before an unfollow?
- **The clock.** Countdowns and "read-only exactly at the lock" need "now". The server's clock decides the lock (T10). In fixture mode it is a fake clock that only moves when advanced, so it can be hours off the browser's clock. Client clocks can also drift in production.
- **Points per pick.** *Everyone's picks* sorts by points after settlement, but `GET …/picks` (T10) has no points; T11 left the breakdown out of the API.
- **Overdue matches.** A match whose start passed but that sync has not reported live yet (up to `live_every`, 2 min by default) is neither upcoming nor live.

## Decision

1. **Full** = a winner, a series score (Bo3/Bo5), and a map winner for every map the series implies (2:1 → maps 1–3, 2:0 → maps 1–2; Bo1 → map 1). Map scores stay optional. Anything less is *partial*. `domain::picks::completeness` implements it.
2. **Needs my pick** = pickable and no pick at all. The filter runs in the frontend over the Upcoming list.
3. **Only followed events are listed.** The match detail and `GET …/picks` open for any match, so a member can still see a pick after an unfollow.
4. **Segments**, all on the server clock (`AppState::clock`):

   | Segment | Statuses | Window | Order | Paging |
   | --- | --- | --- | --- | --- |
   | `upcoming` | `scheduled`, `postponed` | `scheduled_at <= now + 14 d`; no start time → not listed | start, ascending | one page (up to 200) |
   | `live` | `live` | – | start, ascending | one page (up to 200) |
   | `finished` | `finished`, `canceled` | `coalesce(ended_at, started_at, scheduled_at) >= now - 30 d` | that time, descending | pages of 30 |

   Overdue matches stay in Upcoming, shown as locked, until sync reports them live. The cursor is the keyset `(sort time in ms, match id)`, written `<ms>_<id>`; clients treat it as opaque. Bad `state`, `event_id` or `cursor` answer 422 `invalid_query`. `following_any` tells the frontend whether to show the admin hint (`/event follow`, T09).
5. **Server time header.** Every `/api` response, 304s and errors included, carries `X-Server-Time` (RFC 3339 UTC, milliseconds) from `AppState::clock`. The middleware sits outside the ETag layer, so bodies don't change and polls still get 304s. The frontend (`clock.ts`) keeps the offset to `Date.now()`, ignoring changes under 1 s. Countdowns and the lock timer (`useLockPassed`, a `setTimeout` to the exact lock moment) run on server time. Without the header the offset stays 0.
6. **Lock in the UI.** The form turns read-only at whichever comes first: the lock timer, `pickable: false` from a poll of `GET …/picks`, or a `pick_locked` answer to a save. The server still decides; the UI only follows.
7. **Points on `GET …/picks`.** Every pick gets `result: {points, state, pending_parts, breakdown} | null`, read from `pick_results` for the caller's guild. The frontend sorts by points once any result exists (ties by name, unsettled picks last).
8. **Polling.** Upcoming and Live poll every 20 s, Results every 60 s. The detail polls both of its routes every 20 s while the match is live or locks within 24 h, else every 60 s.

## Consequences

- A match moves Upcoming → Live → Results within one poll of the sync writing it. `api/src/matches/tests.rs` drives this with the fixture source.
- The soft hint for map picks that disagree with the series or winner pick lives in the frontend only (`pickHints`), as ADR 0010 left it.
- In fixture mode the browser follows the fake clock after the first response. Between polls it runs on in real time while the server's clock stays frozen, so the lock timer may fire before the server locks. The form then shows read-only a little early; the server's answer stays the authority.
- The Upcoming list has no time-less matches (TBD dates). T13's event view shows the whole event.
- Follow-ups: T13 can reuse `GET /api/matches?event_id=` and the row components; T15 can reuse `summary.ts` and the breakdown view.
