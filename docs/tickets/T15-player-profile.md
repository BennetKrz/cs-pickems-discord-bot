# T15 – Player profile and pick history

- **Epic:** D – Activity views
- **Status:** in review
- **Index:** [all tickets](README.md)

**Goal:** each member sees their own score and the matches they picked, and can look at anyone else's.

**Depends on:** T11, T14

**Scope**

- The *Me* tab is the current user's profile. The same view opens for any member from the leaderboard or a pick list, because picks are public.
- Header: rank, points, number of picks, accuracy per part: winner, exact series, map winner, exact map.
- Lists: **Open** (editable, link to the match), **Live** (provisional points), **Settled** (points breakdown per match). Filter by event.
- API: `GET /api/users/{id}/profile`, `GET /api/users/{id}/picks?state=&event_id=&cursor=`.

**Acceptance criteria**

- [x] Profile numbers equal the leaderboard numbers for the same filter.
- [x] Void picks appear in history marked *void* and are excluded from accuracy.
- [x] Another member's profile is reachable in two taps from the leaderboard.

**Notes**

- Decisions are in [ADR 0018](../adr/0018-player-profile.md). The owner approved these defaults:
  - the header shows all picks for the filter and the leaderboard's settled picks;
  - the lists cover every event, like the leaderboard; the filter lists followed events;
  - a user with no `guild_members` row and no picks in the caller's guild is a 404;
  - accuracy counts parts decided during a live match, like the leaderboard's winner accuracy.
- Also decided:
  - the header is the member's row of `store::leaderboard::load`, so the numbers match by construction;
  - `profile` also takes `event_id`;
  - a match voided in any status goes to Settled;
  - the list opens on Live if a pick is live, else Open, else Settled;
  - void picks are tagged *Void* without per-part outcomes.
- Dependencies: T11 and T14 still say *in review* in the index but are merged to `main`; T15 builds on them as merged.
- Code:
  - domain: `Standing` gains `series_decided`, `map_winner_hits`/`_decided` and `map_score_decided`, the accuracy helpers, and `leaderboard::rate`;
  - store:
    - `store::profile::{list_picks, pick_counts, is_member}`;
    - `list_picks` reuses `list_sql!` (now `#[macro_use]` on `mod matches`) and `matches::list_item_from_row`;
  - API:
    - `backend/crates/api/src/profile.rs`;
    - `matches::item_header` (shared with `RowContext::row`), `encode_cursor`/`decode_cursor` and `PartsDto::from_parts` became `pub(crate)`;
  - frontend:
    - `src/members/` (`ProfileView`, `MemberProfile`, `format.ts`, `link.ts`);
    - the Me tab (`routes/tabs.tsx`) uses `ProfileView`;
    - leaderboard rows and *Everyone's picks* link to profiles through `profileLink`, with the event filter and a back link;
    - `StatusTag` takes a `MatchHeader`;
    - types in `api.ts`.
- Tests:
  - `domain::leaderboard`: counts per part by outcome, and void results skipped;
  - `store/tests/profile.rs`:
    - every status in one list (voided while scheduled or live goes to Settled), and the order;
    - cursor paging;
    - unfollowed events, the event filter, user and guild isolation;
    - `pick_counts` and `is_member`;
  - `api/src/profile/tests.rs` (settled by the real `sync::settle`):
    - **AC 1:** every board row equals its profile, all time and per event;
    - **AC 2:** a match voided after it was settled moves the winner accuracy from 1/2 to 1/1 and stays listed as `void`; a match voided before it started is Settled, not Open;
    - lists, provisional points, pickable and locked on the server clock;
    - pages of 30;
    - 401, 404 and 422;
  - frontend:
    - `ProfileView.test.tsx`:
      - header and accuracy tiles, an unranked member;
      - the three lists with outcomes per part, the void tag;
      - *Edit pick* only on my own pickable picks;
      - the event filter (also unfollowed), load more, 20 s polling while live, retry;
      - my own member page goes to Me;
      - **AC 3:** Leaderboard tab, then a row, shows the profile;
      - opening from *Everyone's picks* and going back;
    - `LeaderboardView.test.tsx`: links carry the filter, and back returns to the filtered board;
    - `format.test.ts`.
- Checked by hand on a dev build in fixture mode (API, no browser):
  - two members picked two Bo1s, which went live and then finished;
  - leaderboard and profile agreed at every step, all time and with `?event_id=1`;
  - after `POST /api/admin/matches/2/void`, match 2 was listed under Settled as `void`, and the winner accuracy went from 1/2 to 1/1;
  - 404 for an unknown user, 422 for a bad `state` or `event_id`.
- Not checked: the layout in the Discord client and on a phone.
- Follow-ups:
  - **P6** (streaks, achievements) and **P3** (seasons) can extend this view; a season filter would bound header and lists alike.
  - If the guild grows large, cache the board per guild (shared with T14).
