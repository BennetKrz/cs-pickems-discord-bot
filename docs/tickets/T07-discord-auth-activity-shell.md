# T07 – Discord application, authentication and Activity shell

- **Epic:** B – Discord integration
- **Status:** done
- **Index:** [all tickets](README.md)

**Goal:** a member opens the Activity in the server and is signed in with their Discord account automatically.

**Depends on:** T01, T02

**Scope**

- `docs/discord-setup.md`: one Discord application with a bot user and Activities enabled. URL mapping `/` points to the public hostname (your domain, T16). Add a placeholder OAuth2 redirect URI. Install with scopes `bot` and `applications.commands`.
- **Auth flow:** the frontend runs `discordSdk.ready()`, then `commands.authorize` (scope `identify`), and gets a code. It calls `POST /api/auth/token`. The backend exchanges the code with the client secret, loads `/users/@me`, and checks with the bot token that the user is a member of the guild. The guild ID from the SDK is a client claim; it must equal `DISCORD_GUILD_ID` when set. The backend then creates a server-side session (opaque token, 7 days) and returns it with the Discord access token. The frontend calls `commands.authenticate`.
- The session travels as `Authorization: Bearer`, not as a cookie, because the app runs in a third-party iframe.
- **Admin check:** the user has *Manage Server* in the guild, or the role set via `/admin role`. Computed from guild roles via the bot and cached 5 min. Used by every admin API and command.
- Upsert `users` and `guild_members` (name, avatar) on login. This is how a person becomes a player: there is no separate sign-up.
- **Activity shell:** tabs *Matches*, *Events*, *Leaderboard*, *Me*. Responsive down to phone width, because the Discord mobile client runs Activities too. Dark theme close to Discord's, with loading, empty and error states.
- **CSP:** the Activity can only reach hosts through Discord's proxy. Serve team logos and avatars through a backend image proxy `/api/img` with an allow-list of hosts and a disk cache, so no extra URL mappings are needed.
- **Freshness:** poll with ETag/`If-None-Match`, every 20 s on views that show live matches and 60 s elsewhere. Pause while the document is hidden.
- Launched outside a guild (DM): show "Open this from the server" and nothing else.

**Acceptance criteria**

- [x] The Activity opens in the test server on desktop and mobile and shows the user's name and avatar.
- [x] A forged guild ID, a non-member, and an expired session all get 401/403; tests cover each.
- [x] Admin detection is correct for the owner, a *Manage Server* role, the configured role, and a normal member.
- [x] No CSP errors in the Activity console.

**Notes**

- Decisions are in [ADR 0007](../adr/0007-discord-auth-sessions.md):
  - sessions are stored as SHA-256 hashes (migration `0004_sessions.sql`), with a fixed 7 days on the real clock;
  - scope `identify` only, with membership checked by the bot token;
  - the client ID comes from `GET /api/auth/config` instead of a build-time variable;
  - the admin rule is in `domain::admin`, with Discord data cached for 5 min, the configured role read fresh, and a 30 s backoff after Discord failures;
  - the image proxy is keyed by internal IDs (`/api/img/avatar/{user_id}`, `/api/img/team/{team_id}`) instead of taking URLs, so source URLs with external IDs never reach the client; raster images only;
  - an ETag middleware covers every JSON `GET` under `/api`;
  - dev builds get a dev sign-in (`POST /api/dev/session`), which `pnpm dev` uses outside Discord (owner's choice).
- Setup guide: [docs/discord-setup.md](../discord-setup.md). New setting: `IMAGE_CACHE_DIR` (default `data/img-cache`).
- Tests:
  - `backend/crates/api/src/auth/tests.rs`: forged, missing and foreign guild; non-member; bad code; Discord outage; expiry at 7 days; malformed tokens; pinning the guild; the admin rule over HTTP for owner, *Manage Server*, the configured role and a member; cache and backoff.
  - `auth/discord.rs`: the real client against a local mock (form body, bot token, User-Agent, 404/403).
  - `img/tests.rs`: allow-list, disk cache, stale copies, non-images, redirect and size limits.
  - `etag.rs`, `domain::admin`, `store` session repository and constraints.
  - Frontend: `discord.test.ts` (SDK call order and arguments), `auth.test.tsx` (outside a server, errors, re-sign-in on 401), `poll.test.tsx` (ETag, 20/60 s, pause while hidden), `shell/Shell.test.tsx`.
- Checked locally with a dev build and `pnpm dev` in headless Chrome at 360 px and 1100 px: sign-in, tabs, the avatar through the proxy, 304s, and "Open this from the server" for the production build outside Discord.
- Checked inside Discord by the owner on 18 Nov 2026, with a development app and a cloudflared quick tunnel:
  - the desktop client and the mobile app open the Activity and show the name and avatar;
  - the console of the Discord web client (Firefox, production build) has no CSP errors from the Activity. The only CSP messages come from discord.com itself (one blocked script injected by a browser extension), plus harmless rejections of the tunnel's `__cf_bm` cookie.
- Follow-ups:
  - T08: `/admin role set` writes `guild_settings.admin_role_id`; bot admin checks can reuse `domain::admin` with interaction data. The deep link (pending view) goes into the shell's memory router.
  - T12–T15: replace the placeholder tabs in `frontend/src/routes/tabs.tsx`; use `usePoll` with `POLL_LIVE_MS` or `POLL_IDLE_MS`, and `api::img::team_logo_url` for logos.
  - T16: rate-limit `/api/auth/*`; set `IMAGE_CACHE_DIR=/data/img-cache` in the image.
  - Discord shows an unverified Activity only to the app's team and up to 50 app testers, in servers under 25 members. This is an open question in [vision.md](../vision.md#open-questions).
  - The member's server avatar is not stored; the user avatar is shown.
