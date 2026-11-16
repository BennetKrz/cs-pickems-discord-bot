# T07 – Discord application, authentication and Activity shell

- **Epic:** B – Discord integration
- **Status:** todo
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

- [ ] The Activity opens in the test server on desktop and mobile and shows the user's name and avatar.
- [ ] A forged guild ID, a non-member, and an expired session all get 401/403; tests cover each.
- [ ] Admin detection is correct for the owner, a *Manage Server* role, the configured role, and a normal member.
- [ ] No CSP errors in the Activity console.
