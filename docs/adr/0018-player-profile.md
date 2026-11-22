# 0018 – Player profile and pick history

- **Status:** accepted
- **Date:** 2026-11-22
- **Ticket:** T15

## Context

T15 asks for a profile per member: a header with rank, points, number of picks and accuracy per part (winner, exact series, map winner, exact map), then **Open**, **Live** and **Settled** lists with an event filter. It names two routes, `GET /api/users/{id}/profile` and `GET /api/users/{id}/picks?state=&event_id=&cursor=`. Profile numbers must equal the leaderboard's for the same filter, and void picks must appear in the history but not count for accuracy. Some things were open:

- **Accuracy per part.** The leaderboard (ADR 0016) counts hits and decided parts only for the winner. For series and maps it counts hits but not decided parts.
- **"Number of picks".** Every pick, or only the settled ones the leaderboard counts?
- **What each list holds**, and where a voided or canceled match goes.
- **Which events.** The match list (T12) shows followed events only. The leaderboard counts every event.
- **Who can be opened.** Picks are public (vision), but `{id}` is any string.

## Decision

1. **The header is the member's row of the leaderboard.**
   - `GET /api/users/{id}/profile?event_id=` calls `store::leaderboard::load` for the same guild and filter and picks the member's row: rank, previous rank, points, provisional points and picks, settled picks.
   - The numbers equal the leaderboard's by construction.
   - A member without a non-void result has `rank: null` and zeros.
2. **Accuracy per part.** `domain::leaderboard::Standing` also counts `series_decided`, `map_winner_hits`, `map_winner_decided` and `map_score_decided`, under the existing rule:
   - only `hit` and `miss` parts count;
   - `pending` and `void` parts count neither way;
   - void results count for nothing.

   The API sends each part as `{hits, decided, rate}`. The tie-break and ranks are unchanged.
3. **Pick counts.** The header shows both numbers:
   - all picks for the filter (`picks.total`, plus `open`, `live` and `settled` for the list tabs);
   - the leaderboard's settled picks.
4. **Lists** (`store::profile::list_picks`), only matches the member picked in the caller's guild:

   | List | Matches | Order | Page |
   | --- | --- | --- | --- |
   | `open` | `scheduled` or `postponed`, not voided | scheduled start, ascending (no time first) | 200 |
   | `live` | `live`, not voided | start, ascending | 200 |
   | `settled` | `finished` or `canceled`, or voided in any status | end, else start, else scheduled; latest first; no time window | 30 |

   The cursor is the match list's keyset `<ms>_<id>` (ADR 0012). Each entry has the match header (pickable and locked on the server clock, as in T12), the member's parts, and `pick_results` if settled (points, state, pending parts, breakdown).
5. **All events, not only followed ones**, as on the leaderboard, so the lists and the header agree. The filter lists followed events. An unfollowed event in the URL still works.
6. **Membership.** Any member of the caller's guild can open any other member's profile. Anyone without a `guild_members` row and without picks in that guild gets 404 `not_found`. Bad `state`, `event_id` or `cursor` gets 422 `invalid_query`.
7. **Activity:**
   - One view (`members/ProfileView`) serves the Me tab and `/members/:id`. `/members/<my id>` redirects to `/me`.
   - Leaderboard rows link to the profile with the board's event filter. Names in *Everyone's picks* link to the profile too.
   - The back link returns to where the profile was opened.
   - The list opens on Live if any pick is live, else Open, else Settled.
   - Open rows on your own profile show *Edit pick* while pickable and link to the match.
   - Settled rows show each part's outcome. Void picks are dimmed and tagged *Void*, without per-part outcomes.
   - The view polls every 60 s, every 20 s while a picked match is live.

## Consequences

- No migration. The profile reads the whole guild's settled picks to rank one member, like the leaderboard does. That is fine for one server; caching per guild (ADR 0016) would serve both.
- A match without any time (no end, start or schedule) has no cursor key. It is listed, but paging stops at it. Settled matches always have times in practice.
- Achievements and streaks (P6) and seasons (P3) can add to this view. The season filter would bound `match_at` the same way for the header and the lists.
