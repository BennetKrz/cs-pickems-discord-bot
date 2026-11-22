# T09 – Followed events

- **Epic:** B – Discord integration
- **Status:** done
- **Index:** [all tickets](README.md)

**Goal:** admins decide which tournaments are synced and pickable. This keeps both quota and noise low.

**Depends on:** T06, T08

**Scope**

- `/event search <name>` lists results from all adapters with search capability: name, tier, dates, source. Results are cached 24 h. A select menu follows the chosen event.
- `/event follow`, `/event unfollow` (asks for confirmation when picks exist), `/event list`.
- Following triggers an immediate structure sync (T06) and makes the event's matches pickable once teams are known.
- Unfollowing stops syncing and hides the event from *Upcoming*. Existing picks, points and history stay unchanged.
- Only matches of followed events appear anywhere in the app.

**Acceptance criteria**

- [x] With the fixture source, a followed event's matches appear in the Activity within 1 min.
- [x] Unfollowing does not change the leaderboard.
- [x] A search costs at most one request per source and is served from cache on repeat.

**Out of scope:** automatic following by tier (the owner chose admin-curated events).

**Notes**

- Decisions are in [ADR 0014](../adr/0014-followed-events.md). The owner approved these defaults in the plan:
  - `/event follow <event>` autocompletes events already in the database; new events come in through `/event search`;
  - the dev fixture source is a search-only scheduler source (`SourceEntry.search_only`);
  - a search result from a non-primary source that isn't linked yet is refused, with a pointer to `/admin link`;
  - no HTTP endpoints for follow/unfollow in T09.
- Code:
  - `sync::follow`: `follow_hit` (ingest the result, resolve, follow), `follow`, `unfollow`. Audit actions are `follow_event` and `unfollow_event`; `sync_event_now` runs after a new follow.
  - `bot::commands::event`: the `/event` group behind the admin check, with a select menu and confirm buttons answered by a collector (5 min and 2 min). Custom IDs are `event:<interaction>:select|confirm|cancel`.
  - `bot::render::events`: the texts.
  - Store: `events::search_known`, `guilds::followed_overview`, `picks::count_in_event`.
  - `FixtureDriver::adapter()`, which the app passes to the scheduler as search-only in dev.
- "Only matches of followed events appear anywhere" already held for the lists, the digest, result posts and `/mypicks`, which all join `followed_events`. The match detail still opens for an unfollowed event (T12), so links and history keep working.
- Tests:
  - `sync/tests/follow.rs`:
    - follow from search, then the structure sync on the next tick;
    - following twice is idempotent;
    - `NeedsLink`;
    - an unknown event;
    - **AC 3**: one search request per source, then the cache, also with different case and spacing;
    - a search-only source is never polled;
    - **AC 2**: settled points, unfollow, the leaderboard is unchanged, picks stay, the event leaves Upcoming and polling stops.
  - `api` `matches/tests.rs` (**AC 1**): with `FixtureDriver` plus the search-only fixture source, search → `follow_hit` → `GET /api/matches?state=upcoming` lists the fixture matches, some of them pickable, and only in that guild. The Activity polls this list every 60 s.
  - `store/tests/repos.rs`: `search_known` (filters, `LIKE` escaping, limit), `followed_overview` and `count_in_event`.
  - `bot`: every `/event` leaf is behind the admin check; renderers (25-item cap, plain dates in options, no "bet"); component ID parsing.
- **Checked in Discord by the owner on 22 Nov 2026** (select menu, confirm buttons), as in T08:
  - `cargo run --features dev` with `ENABLED_SOURCES=fixture,manual`;
  - `/event search` with a word from the fixture event's name, then choose it: the matches appear in the Activity;
  - `/event list`;
  - pick a match, then `/event unfollow`: the bot asks first, and `/leaderboard` is unchanged.
- Follow-ups:
  - P5: HTTP follow/unfollow on `sync::follow`, if the admin screen wants it.
  - After a restart, an open search menu is dead (Discord shows "interaction failed"); searching again is served from the cache.
