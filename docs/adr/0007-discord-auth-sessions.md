# 0007 – Discord sign-in, sessions, admin check and image proxy

- **Status:** accepted
- **Date:** 2026-11-17
- **Ticket:** T07

## Context

T07 signs members in through the Activity, adds the admin rule, and builds the Activity shell. The ticket fixes the flow: `ready` → `authorize` → `POST /api/auth/token` → `authenticate`, with a server-side opaque session of 7 days sent as a bearer token. It also fixes the admin rule (*Manage Server* or the `/admin role` role, cached 5 min), an image proxy with an allow-list and disk cache, and ETag polling. Facts that shaped the details (Discord docs checked 17 Nov 2026):

- Activities reach only their own origin `https://<client id>.discordsays.com`, which proxies to the URL mapping. Relative `fetch('/api/…')` works; the old `/.proxy` prefix is not needed.
- The token endpoint takes form data only, with `client_id` and `client_secret`. Embedded apps send no `redirect_uri`.
- `GET /guilds/{g}/members/{u}` with the bot token needs no privileged intent. `GET /guilds/{g}` returns `owner_id` and the roles with permissions as decimal strings. `MANAGE_GUILD = 1 << 5`, `ADMINISTRATOR = 1 << 3`, and the `@everyone` role has the guild's ID.
- `@discord/embedded-app-sdk` 2.5.0 throws in its constructor without the `frame_id` query parameter, so it can't run in a plain browser. `guildId` is `null` in DMs.
- The architecture says external IDs never appear in the public API, yet stored team logo URLs contain PandaScore IDs.
- The fixture dev clock (T05) can jump days ahead.

## Decision

1. **Sessions.**
   - The token is 32 bytes from the OS RNG (`getrandom`) as 64 hex digits. The `sessions` table (migration 0004) stores only its SHA-256, with user, guild, `created_at` and `expires_at`.
   - The lifetime is a fixed 7 days, not sliding. Expired rows are purged at each sign-in.
   - Each request re-checks the session's guild against `DISCORD_GUILD_ID`, so pinning the guild later ends other guilds' sessions.
   - Sessions use the system clock even in fixture mode, so advancing the dev clock never signs anyone out.
2. **Sign-in checks, in this order.**
   1. The guild claim must be present (400 `guild_required`) and equal `DISCORD_GUILD_ID` when set (403 `wrong_guild`), before any Discord call.
   2. The code exchange (bad code: 401 `invalid_code`) and `/users/@me`.
   3. Membership via the bot token (absent, or bot not in the guild: 403 `not_a_member`).
   4. Then the `users` and `guild_members` upsert (with the server nickname) and the session, in one write transaction.

   Discord errors are 503 `discord_unavailable`. The scope is `identify` only. The answer is `Cache-Control: no-store` and includes the Discord access token for `commands.authenticate`; the server keeps no copy.
3. **The frontend keeps the session in memory only.** It runs the SDK flow at every launch (`prompt: 'none'` makes it silent after the first consent). On a 401 it signs in again once, in the background; a second 401 within a minute shows the error state.
4. **The client ID comes from `GET /api/auth/config`,** not from a build-time `VITE_` variable, so one image on GHCR (T16/T17) serves any Discord application.
5. **The admin rule is a pure function in `domain::admin`:** owner, or base permissions (`@everyone` plus member roles) with *Manage Server* or *Administrator*, or the configured role.
   - The `api` crate's `AuthService::is_admin` caches Discord's answers for 5 min: guild facts per guild, member roles per (guild, user), including "not a member". Errors are not cached.
   - `guild_settings.admin_role_id` is read fresh each time, so `/admin role` (T08) takes effect at once.
   - After a Discord failure, Discord is not asked again for 30 s; admin routes answer 503 meanwhile, and `/api/me` reports `is_admin: false`.
   - Rejected credentials are logged as an error once, then at debug level.
   - T08's bot can apply the same `domain::admin` rule to the member data in interactions.
6. **The image proxy is keyed by internal IDs:** `GET /api/img/avatar/{user_id}` and `/api/img/team/{team_id}`. The backend looks the upstream URL up in the database, so the proxy is not open and source URLs never reach the client. API responses carry `?v=` cache-busting URLs (the avatar hash, or a short hash of the logo URL).
   - Upstream URLs must be HTTPS on the default port, on an allow-listed host (`cdn.discordapp.com`, `cdn-api.pandascore.co`), including every redirect (at most 3).
   - Only raster images are served: PNG, JPEG, GIF, WebP and AVIF, recognized by magic bytes, up to 2 MB, with our own content type and `nosniff`. SVG is rejected because it can carry scripts.
   - The disk cache is `IMAGE_CACHE_DIR/<sha256 of URL>`, written atomically and refetched after 7 days. A stale copy is served when the upstream fails. There is no size cap: a few hundred teams and members is a few MB.
   - The routes need no session, because `<img>` can't send a bearer token. They reveal only avatars of known users and logos of known teams.
7. **ETags are an axum middleware on `/api`.** Every `GET` with status 200 and a JSON body gets a strong ETag (a truncated SHA-256 of the body) and `Cache-Control: private, no-cache`. A matching `If-None-Match` gets 304. The frontend sends `If-None-Match` itself, with `cache: 'no-store'`, so the browser cache can't interfere.
8. **Dev sign-in, dev builds only.** `POST /api/dev/session {user_id, guild_id?, username?}` creates a real session without Discord. It still applies the guild rule and keeps a stored profile. `pnpm dev` uses it outside Discord as `VITE_DEV_USER_ID` (default `200000000000000001`). `X-Dev-User` stays valid for admin routes and falls back to the session when absent.
9. **Calling Discord from the API is an exception** to "only the sync worker calls external APIs": the rule protects data-API budgets, while sign-in, the admin check and the image proxy must call Discord or a CDN. Each such call is bounded by a cache.

## Consequences

- `DenyAll` is gone. `AppState::new` uses an `AuthService` without Discord, so tests that don't sign in need no setup, and admin routes answer 401.
- Membership is verified at sign-in, and on admin checks at most every 5 min, not on every member request. A member who leaves the server keeps read access until the session ends (at most 7 days).
- The member's server avatar is not stored (`guild_members` has no column for it); the user avatar is shown.
- `/api/auth/*` is not rate-limited yet (T16).
- New allow-listed hosts are a code change in `api::img::ALLOWED_HOSTS`, for example when a second data source with its own CDN arrives.
- Discord limits who can launch an unverified Activity (team, app testers, small servers); see `docs/discord-setup.md`.
