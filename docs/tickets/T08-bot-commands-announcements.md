# T08 – Bot commands, channel announcements and Activity launch

- **Epic:** B – Discord integration
- **Status:** done
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

- [x] Driving the fixture lifecycle (T05) produces the digest and result posts once each, even across a restart.
- [x] Non-admins get an ephemeral permission error for every admin command.
- [x] *Pick now* opens the Activity on the right match.

**Notes**

- Decisions are in [ADR 0013](../adr/0013-bot-announcements-launch.md). The owner chose:
  - the digest schedule per guild via `/admin digest set <HH:MM> <IANA zone>`, with defaults 10:00 and `Europe/Berlin`;
  - result bundles that wait 10 min after the first finish, so the best pickers are final.
- Discord docs checked on 22 Nov 2026:
  - `LAUNCH_ACTIVITY` (12) works for commands and buttons;
  - the Entry Point command is global only, so guild commands don't touch it;
  - **no launch parameters are documented**, so the deep link uses the pending view;
  - `nonce` with `enforce_nonce` deduplicates sends.
- Libraries: poise 0.7.1 on serenity 0.12.5 (which has `CreateInteractionResponse::LaunchActivity`), and chrono-tz 0.10.4.
- Migration `0006_bot.sql`:
  - `guild_settings.digest_time`, `timezone` and `announce_since`;
  - `bot_posts`, `announced_results` and `pending_views`.
- New API: `GET /api/me/pending-view` returns `{ match_id }` and takes the view (one use). The frontend's `PendingViewOpener` calls it once per sign-in and opens `/matches/:id`.
- Beyond the ticket's list:
  - `/admin match unvoid`, because T05 added unvoid;
  - `/admin digest set`, the owner's choice;
  - `/admin link list | merge | create`.
- `/admin channel set` starts `announce_since`, so results from before are not posted.
- `/admin match result` checks that the map scores agree with the series score before writing anything.
- `/event …` is T09's: it adds its commands under the same framework (`bot::commands`).
- Tests:
  - `bot/tests/announce.rs`:
    - the whole fixture scenario with picks, settlement and restarts every 6 h: one digest, every finished match announced exactly once, the canceled one never, at most 6 posts per hour, best pickers named;
    - a lost send answer resent with the same nonce;
    - the rate limit across a restart;
    - bundling (8 vs 12 min);
    - no channel, `announce_since`, and the digest time, day and time zone.
  - `bot/tests/launch.rs`: the pending view from *open* buttons.
  - `bot` unit tests:
    - every `/admin` leaf is behind the check;
    - the admin rule on interaction data;
    - the renderers (no "bet", `<t:>` timestamps, ties);
    - DST and local days, nonces, and score parsing.
  - `domain::admin::interaction_rule_table`.
  - `store/tests/bot.rs`: claims, rate counts, announced results, top pickers, pending views and settings setters.
  - `api` `me.rs`: the pending view is used once, per member and guild, expires after 10 min, and needs a session.
  - Frontend `pendingView.test.tsx`: opens the match, asks once under StrictMode, and ignores failures.
- **Checked in Discord by the owner on 22 Nov 2026 (AC 3)**, with a dev build and the fixture source:
  - `cargo run --features dev` with `ENABLED_SOURCES=fixture,manual` and a dev bot token;
  - follow the fixture event (until T09: insert into `followed_events`);
  - `/admin channel set`;
  - the digest appears (advance the dev clock with `POST /api/dev/clock/advance` if needed);
  - click *Pick …*: the Activity opens on that match;
  - restart the app: no duplicate posts.
- Follow-ups:
  - edit a result post when its result is corrected or voided later;
  - T14: replace the `/leaderboard` sum with its tie-break query;
  - P1: channel reminders as a new `bot_posts.kind` under the same rate limit.
