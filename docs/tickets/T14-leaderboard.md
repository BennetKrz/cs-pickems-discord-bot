# T14 – Leaderboard

- **Epic:** D – Activity views
- **Status:** in review
- **Index:** [all tickets](README.md)

**Goal:** show who in the server leads.

**Depends on:** T11, T07

**Scope**

- Ranks every member with at least one settled pick: rank, avatar, name, points, settled picks, winner accuracy, exact series hits, exact map hits, and rank change since the previous match day.
- **Tie-break:** points, then exact hits (series plus map), then winner accuracy; still equal means a shared rank.
- Provisional points from live matches count and are marked as such.
- Filter: all time, or one followed event.
- The current user is highlighted, with a *jump to me* control. A row opens that member's profile (T15).
- API: `GET /api/leaderboard?event_id=`. The bot's `/leaderboard` (T08) uses the same query.

**Acceptance criteria**

- [x] Tie-break and shared ranks are unit-tested.
- [x] A settlement change shows up within one poll interval. *(Polls every 60 s, every 20 s while a match is live.)*
- [x] Points per member equal the sum of their `pick_results`, checked in a test.

**Notes**

- Decisions are in [ADR 0016](../adr/0016-leaderboard.md). The owner chose:
  - **rank change as a recompute, no snapshots:**
    - a match day is a calendar day in the guild's time zone (default Europe/Berlin), dated by match start;
    - the previous rank comes from the current `pick_results` without the latest match day's matches;
  - **rows open a placeholder profile:** your own row opens `/me`, others open `/members/:id` until T15.
- Also decided:
  - members with at least one non-void result appear; *settled picks* counts those;
  - provisional points count, and the UI shows them as "+N live";
  - winner accuracy counts `hit`/`miss` winner parts only;
  - exact map hits are map `score` parts with `hit`;
  - the tie-break compares accuracy as an exact fraction (none decided = 0);
  - shared ranks are competition style (1, 1, 3), ordered by name inside;
  - the response lists the followed events for the filter;
  - the board polls every 60 s, every 20 s while `live`.
- Dependencies: T11 (and T12) still say *in review* in the index but are merged to `main`; T14 builds on them as merged.
- Code:
  - domain:
    - `domain::leaderboard` (`tally`, `compare`, `rank`, `build`);
    - `domain::localtime`, moved from `bot::schedule`, which re-exports it;
  - store: `store::leaderboard::{scored_picks, members, filter_events, load}`; it replaces `store::bot::display_names`;
  - API: `GET /api/leaderboard?event_id=` in `backend/crates/api/src/leaderboard.rs`;
  - bot: `/leaderboard` uses `store::leaderboard::load` (top 10); `render::leaderboard` takes ranked `LeaderLine`s and marks live points;
  - frontend:
    - `src/leaderboard/` (`LeaderboardView`, `format.ts`);
    - the placeholder `src/members/MemberProfile.tsx`;
    - routes `leaderboard` and `members/:id`;
    - types in `api.ts`.
- Tests:
  - `domain::leaderboard`: points first, exact hits, accuracy fractions, shared ranks (1, 1, 3), the order inside a rank, the tally by outcome, void-only members, match days across DST and the guild's day, previous ranks;
  - `store/tests/leaderboard.rs` (**AC 3**): points equal `sum(pick_results.points)` per member, and agree with `totals_for_guild`; also the event filter, guild isolation, names and avatars, the guild time zone, the followed events, and an unreadable breakdown;
  - `api/src/leaderboard/tests.rs`:
    - **AC 2:** after a real re-settlement, the next `GET` has the new points and a new ETag;
    - provisional points while live through `sync::settle`;
    - the filter, guild isolation, names, 401 and 422;
  - bot render: given ranks and live points;
  - frontend:
    - `LeaderboardView.test.tsx`:
      - ranks, markers and stats;
      - your own row highlighted, and *jump to me*;
      - profile links;
      - the event filter and the empty states;
      - **AC 2 (UI):** the new points appear after one poll interval, not before;
      - 20 s polling while live;
      - the error state;
    - `format.test.ts`.
- Checked by hand on a dev build in fixture mode (API, no browser):
  - two members picked two Bo1s;
  - while live: shared rank 1, `live: true`;
  - after the end: 4 and 3 points, equal to their `pick_results`;
  - `?event_id=1` filters, and `?event_id=x` gives a 422.
- Not checked: the layout in the Discord client and on a phone.
- Follow-ups:
  - **T15:** replace `/members/:id`; show the same numbers for the same filter (reuse `domain::leaderboard::tally`).
  - **P3 seasons:** the event filter and `match_at` can bound a season.
  - If the guild grows large, cache the board per guild and invalidate on settlement.
