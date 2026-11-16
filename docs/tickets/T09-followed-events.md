# T09 – Followed events

- **Epic:** B – Discord integration
- **Status:** todo
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

- [ ] With the fixture source, a followed event's matches appear in the Activity within 1 min.
- [ ] Unfollowing does not change the leaderboard.
- [ ] A search costs at most one request per source and is served from cache on repeat.

**Out of scope:** automatic following by tier (the owner chose admin-curated events).
