# T08 – Bot commands, channel announcements and Activity launch

- **Epic:** B – Discord integration
- **Status:** todo
- **Index:** [all tickets](README.md)

**Goal:** the bot is the doorway into the Activity and the server's ticker. It never duplicates full screens.

**Depends on:** T07, T03 (domain events)

**Scope**

- poise framework; commands registered as guild commands for instant updates.
- **Launch paths:** the app's *Entry Point* command (Discord's app-launcher command) opens the Activity. Buttons on bot posts launch it through the interaction response type for launching an Activity. Verify both mechanisms in the current Discord docs.
- **Deep link:** clicking *Pick now* for match X stores a pending view (user → match X, 10 min TTL). On start, the Activity calls `GET /api/me/pending-view` and navigates there. Use launch parameters instead if Discord supports them by then.
- **Member commands** (ephemeral replies): `/leaderboard` (top 10 embed plus launch button), `/mypicks` (open picks plus launch button). Everything else happens in the Activity.
- **Admin commands:** `/admin channel set`, `/admin role set`, `/admin status` (sync health and quota from T06), `/admin link` (unlinked queue from T03), `/admin match result | clear-override | void | reopen` (T05), and the `/event` commands from T09. Each is checked against the admin rule from T07.
- **Posts** in the configured channel, each with a launch button:
  - Daily digest at a configurable local time (default 10:00, guild time zone): today's pickable matches, lock times, and the pick count so far. The message is edited as the count changes.
  - Match result: final and map scores, plus the best pickers of that match with their points.
  - Rate limit: at most 6 posts per hour. Results that finish within 10 min of each other are bundled into one post.
- Posted message IDs are stored, so a restart never re-posts.
- Needs only *Send Messages*, *Embed Links* and *Use Application Commands*. No privileged intents: member data is fetched by REST for users who log in.

**Acceptance criteria**

- [ ] Driving the fixture lifecycle (T05) produces the digest and result posts once each, even across a restart.
- [ ] Non-admins get an ephemeral permission error for every admin command.
- [ ] *Pick now* opens the Activity on the right match.
