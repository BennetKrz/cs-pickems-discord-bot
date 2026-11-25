# 0013 – Bot commands, channel posts and Activity launch

- **Status:** accepted; decision 2 on the Entry Point command changed by [0019](0019-silent-entry-point.md)
- **Date:** 2026-11-22
- **Ticket:** T08

## Context

T08 turns the bot stub into the real bot: slash commands, launch buttons, and posts in a configured channel (a daily digest and match results), each at most once, also across restarts.

Discord's docs, checked on 22 Nov 2026:

- **Launching an Activity:** the interaction callback `LAUNCH_ACTIVITY` (type 12) works for command, message component and modal interactions, for apps with Activities enabled.
- **Entry Point command:** `PRIMARY_ENTRY_POINT` (type 4) with handler `DISCORD_LAUNCH_ACTIVITY` (2). Discord creates it when Activities are enabled, and only as a global command. An app can't have it as a guild command.
- **Launch parameters:** nothing documented. The SDK reference has no launch-parameter or custom-ID property, and no way to pass data from a button to the launched Activity is documented.
- **Deduplication:** Create Message takes `nonce` (up to 25 characters) and `enforce_nonce`. With both, a repeated nonce from the same author "in the past few minutes" returns the existing message instead of creating a new one.
- **Timestamps:** `<t:UNIX:STYLE>` renders "in the user's timezone and locale".

## Decision

1. **Libraries:** poise 0.7.1 on serenity 0.12.5. serenity has `CreateInteractionResponse::LaunchActivity`, so no raw callback JSON is needed.
2. **Guild commands only:**
   - Commands are registered in `DISCORD_GUILD_ID`, else in every guild from `Ready` and in guilds the bot joins later.
   - The Entry Point command stays Discord's global default. The bot never bulk-overwrites global commands, so it can't delete it.
   - Gateway intents: `GUILDS` only, which is not privileged.
3. **Deep link by pending view:**
   - Buttons have the custom ID `launch` or `open:<match_id>`.
   - Clicking `open:` stores `pending_views(guild, user) → match` for 10 min, then answers `LAUNCH_ACTIVITY`.
   - After sign-in, the Activity calls `GET /api/me/pending-view` once. The call takes the view (it deletes it) and opens `/matches/:id`.
   - A database error still launches the Activity, on the default view.
4. **Admin check from the interaction:**
   - `/admin` carries one poise check, which poise runs for every subcommand. It applies `domain::admin::is_admin_from_interaction` to the interaction member's computed `permissions` and roles, with the configured admin role read fresh from the database.
   - This is the T07 rule without a REST call: Discord gives the owner every permission in interactions, *Administrator* implies all permissions, and *Manage Server* can't be granted or removed per channel.
   - Non-admins get an ephemeral "Only server admins can do this."
   - `default_member_permissions` is not set, so members of the configured role, who may lack *Manage Server*, still see the commands.
5. **Posts are claimed, then sent with a nonce:**
   - A post is a `bot_posts` row, unique on `(guild, kind, key)`. The digest's key is the local date; a result bundle's key is `m<first match ID>`.
   - The row (and a result post's rows in `announced_results`) is inserted in one transaction before sending. The message ID is stored after Discord answers.
   - A claimed row without a message ID (crash, lost answer, Discord error) is resent on the next tick with the same nonce: `pk` + 23 hex characters of a SHA-256 of guild, kind and key.
   - Inside Discord's window this returns the existing message. Outside it, a duplicate is possible only if Discord created the message and the bot never heard back for several minutes, which is accepted.
6. **The announcer reads state, not events:**
   - It ticks every 30 s on the app clock, so it follows the fake clock in fixture mode. `MatchFinished` and `ResultCorrected` only make it tick early.
   - **Results:** finished, not voided matches of followed events, finished after `announce_since`. The finish time is `ended_at`, else when settlement first saw the match finished, else its start.
   - **Bundling (owner's choice):** once the earliest unannounced result is 10 min old, it and every result that finished within 10 min after it go out as one post, at most 10 matches. Settlement has run by then, so the best pickers are final, at the cost of 10 min delay.
   - **Best pickers:** the top 3 point totals above zero, plus ties with the third.
   - **Rate limit:** at most 6 new posts per guild in any rolling hour, counted from `bot_posts.posted_at`, so a restart doesn't reset it. A post exactly an hour old no longer counts. Edits don't count. A post over the limit waits.
7. **Digest schedule per guild (owner's choice):**
   - The digest time and time zone are set with `/admin digest set <HH:MM> <IANA zone>` and stored in `guild_settings.digest_time` and `guild_settings.timezone`. Defaults are 10:00 and `Europe/Berlin`. Zones are validated with chrono-tz.
   - This is the only local time in the backend, and it is used only to schedule. Posts show times as `<t:…>`, so storage and posts stay UTC.
   - The digest is due once the local digest time has passed on that day. A time in a DST gap moves an hour later.
   - It lists the matches whose lock falls in the local day (up to 25), and goes out only if at least one is still pickable.
   - It is edited when its rendered content changes (pick counts, locks), at most once a minute. Edits compare a hash of the rendered post.
8. **`/admin channel set`** also sets `guild_settings.announce_since = now`, so results from before the channel was set are never posted.
9. **`/admin match unvoid`** is included although the ticket's list doesn't name it: T05 added unvoid, and admins otherwise can't undo a void from Discord.
10. **`/admin match result`:**
    - It sets map overrides, then the series override, then status finished, through `ManualService`.
    - Map scores must agree with the series score before anything is written, so a typo doesn't leave a half-applied result.
11. **Gateway failures don't stop the app:** a failed login (e.g. a bad token) is logged and retried with backoff from 5 s up to 5 min. Posts go over REST and don't depend on the gateway.

## Consequences

- New migration `0006_bot.sql`:
  - `guild_settings.digest_time`, `timezone` and `announce_since`;
  - tables `bot_posts`, `announced_results` and `pending_views`.
- `GET /api/me/pending-view` changes state on a `GET`. It is safe, because it is idempotent after the first call and scoped to the session's member and guild.
- `/leaderboard` sums `pick_results` with equal points sharing a rank. T14 replaces it with its tie-break query.
- A corrected result (`ResultCorrected`, a void after posting) does not edit the result post. Follow-up: edit the bundle's message, as the digest is edited.
- Posting needs *Send Messages* and *Embed Links* in the channel. If Discord refuses (403/404), the guild is retried every 5 min and a warning is logged each time.
- The deep link depends on the member clicking and opening within 10 min. If Discord adds launch parameters, the pending view can be replaced without changing the buttons.
- P1's channel reminders count toward the same rate limit through `bot_posts`, with a new `kind`.
