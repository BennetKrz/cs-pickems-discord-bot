# T12 – Matches overview and match detail

- **Epic:** D – Activity views
- **Status:** in review
- **Index:** [all tickets](README.md)

**Goal:** one place to see upcoming, live and past matches with scores, to make picks, and to see everyone else's picks.

**Depends on:** T07, T10, T11

**Scope**

- *Matches* tab with three segments: **Upcoming** (next 14 days), **Live**, **Results** (last 30 days, paged). Grouped by local day, then by event.
- Each row shows logos and names, format, the start time or live score (maps won and current map rounds), event and stage, the pick count, and my status: no pick, partial, full. Finished rows show my points.
- Upcoming has a *Needs my pick* filter and a lock countdown when the start is less than 24 h away.
- **Match detail:** header (teams, format, event, stage, time, status), then a map list (number, map name once known, rounds, winner), then:
  - the **pick form** (T10) while pickable: two team buttons, series-score chips (Bo3/Bo5), a collapsible *Map picks* area with winner plus optional score per map, a *Save* button, and the countdown;
  - **editing:** an existing pick opens pre-filled in the form, with *Save changes* and *Remove pick* buttons, until the lock. Picks under *Everyone's picks* carry an *edited* marker with the time of the last change;
  - otherwise my pick read-only, with the points breakdown once settled;
  - **Everyone's picks:** aggregate bars (winner split, series distribution, per-map split), then every member's pick, sorted by points after settlement. Visible before the lock too.
- Empty state with no followed events: admins see a hint to use `/event follow`.
- API: `GET /api/matches?state=upcoming|live|finished&event_id=&cursor=`, `GET /api/matches/{id}`.

**Acceptance criteria**

- [x] With the fixture lifecycle, a match moves from Upcoming to Live to Results without a reload; live scores appear within 30 s. *(Upcoming and Live poll every 20 s.)*
- [x] The pick form turns read-only exactly at the lock, also if the Activity stayed open. *(A timer at `lock_at` on the server clock, see `X-Server-Time`.)*
- [x] Other members' picks are visible before and after the lock.
- [x] All times show in the viewer's local time zone. *(Frontend tests run with `TZ=Europe/Berlin`.)*

**Notes**

- From T10 ([ADR 0010](../adr/0010-pick-rules-and-api.md)): this ticket takes over the UI halves of two T10 criteria.
  - Edits show in everyone's view within one poll interval: poll `GET /api/matches/{id}/picks` with `usePoll`.
  - The edit controls hide after the lock: use `pickable`/`locked` from that response.
  - Also show the soft hint when map picks and the series pick disagree, e.g. "your map picks add up to 2:1, your series pick is 2:0".
  - Map the error codes `pick_locked`, `not_pickable` and `invalid_pick` to messages.
  - Types and `ApiClient.putPick`/`deletePick` are in `frontend/src/api.ts`.
- Decisions are in [ADR 0012](../adr/0012-matches-views.md). The owner chose:
  - the branch stacks on T11 (still in review);
  - *full* = winner + series (Bo3/Bo5) + a map winner for every map the series implies; map scores optional;
  - *Needs my pick* = pickable and no pick at all;
  - the frontend times countdowns and the lock on the server's clock: every `/api` response carries `X-Server-Time`;
  - the list shows followed events only; the detail opens for any match.
- Also decided:
  - overdue matches (start passed, not live yet) stay in Upcoming as locked;
  - Results include canceled matches; voided ones show *Void*;
  - Results page by 30 with an opaque keyset cursor; Upcoming and Live are one page each;
  - `GET …/picks` gained `result` (points, state, pending parts, breakdown) per pick; the UI sorts by points once results exist.
- Code:
  - rule: `domain::picks::completeness`;
  - store: `store::matches::{list_view, maps_for, Segment, ListQuery}`, `store::picks::{for_user_matches, results_for_guild_match, count_for_match}`;
  - routes: `backend/crates/api/src/matches.rs`; the `X-Server-Time` middleware in `api/src/lib.rs`; `result` in `api/src/picks.rs`;
  - frontend:
    - `src/clock.ts` (server offset, `useServerNow`, `useLockPassed`);
    - `src/matches/` (`MatchesView`, `MatchRow`, `MatchDetail`, `PickForm`, `MyPick`, `EveryonesPicks`, `TeamLogo`);
    - pure helpers: `pickForm.ts`, `format.ts`, `summary.ts`;
    - the route `matches/:id` in `src/router.tsx`.
- Tests:
  - `domain::picks` (completeness over Bo1/Bo3/Bo5);
  - `store/tests/repos.rs` (segments, windows, followed-only, pick counts, event filter, cursor paging, batch queries);
  - `api/src/matches/tests.rs`:
    - segments on the fake clock;
    - the fixture lifecycle through `FixtureDriver`;
    - guild isolation, my status and points, paging;
    - 422s, 401s, 404;
    - ETag 304 plus `X-Server-Time`;
  - frontend:
    - `pickForm.test.ts`, `format.test.ts`, `clock.test.tsx`, `api.test.ts`;
    - `MatchesView.test.tsx`: grouping, filter, Upcoming → Live → Results under polling, paging, empty states;
    - `MatchDetail.test.tsx`: create, edit, remove, the soft hint, read-only exactly at `lock_at`, the server clock offset, `pick_locked`/`invalid_pick`, sort by points, breakdown.
- Checked by hand on a dev build in fixture mode (API, no browser):
  - a full Bo3 pick;
  - advancing the clock moved it from Upcoming to Live (1:0, map 2 current) to Results (2:1);
  - a late `PUT` got `pick_locked`;
  - settlement gave 11 points with the breakdown.
- Not checked: the layout in the Discord client and on a phone.
- Follow-ups:
  - **T09:** the admin hint names `/event follow`; keep the wording in sync.
  - **T13:** reuse `GET /api/matches?event_id=` and `MatchRow`.
  - **T15:** reuse `summary.ts` and the breakdown view from `MyPick.tsx`.
