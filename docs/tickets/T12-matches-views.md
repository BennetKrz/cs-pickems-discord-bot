# T12 – Matches overview and match detail

- **Epic:** D – Activity views
- **Status:** todo
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

- [ ] With the fixture lifecycle, a match moves from Upcoming to Live to Results without a reload; live scores appear within 30 s.
- [ ] The pick form turns read-only exactly at the lock, also if the Activity stayed open.
- [ ] Other members' picks are visible before and after the lock.
- [ ] All times show in the viewer's local time zone.

**Notes**

- From T10 ([ADR 0010](../adr/0010-pick-rules-and-api.md)): this ticket takes over the UI halves of two T10 criteria.
  - Edits show in everyone's view within one poll interval: poll `GET /api/matches/{id}/picks` with `usePoll`.
  - The edit controls hide after the lock: use `pickable`/`locked` from that response.
  - Also show the soft hint when map picks and the series pick disagree, e.g. "your map picks add up to 2:1, your series pick is 2:0".
  - Map the error codes `pick_locked`, `not_pickable` and `invalid_pick` to messages.
  - Types and `ApiClient.putPick`/`deletePick` are in `frontend/src/api.ts`.
