# Discord application setup

One Discord application provides both the bot and the Activity (the embedded web app with all screens). This page walks through the [Developer Portal](https://discord.com/developers/applications) settings it needs and where each value goes. It was checked against Discord's docs on 17 Nov 2026, and the bot parts on 22 Nov 2026 (sources at the end). The portal's menus change from time to time, so look for the setting by name if a path no longer matches.

Use two applications: one for **production** (URL mapping to your domain) and one for **development** (URL mapping to a dev tunnel). Each has its own IDs and secrets.

## 1. Create the application

1. In the [Developer Portal](https://discord.com/developers/applications), click **New Application** and name it (e.g. "CS2 Pick'em"). If others will work on it, create it under a developer team, because only the owner and team members can launch an Activity that is not yet public (see [Who can launch it](#who-can-launch-it)).
2. **General Information**: copy the **Application ID**. This is `DISCORD_CLIENT_ID`.

## 2. OAuth2

1. **OAuth2 → Client Secret**: click **Reset Secret** and copy it. This is `DISCORD_CLIENT_SECRET`, an environment variable only, never in a file in the repo.
2. **OAuth2 → Redirects**: add `https://127.0.0.1` as a placeholder and save. The Embedded App SDK handles the redirect itself, but Discord expects at least one redirect URI.

Sign-in asks only for the `identify` scope. Membership in the server is checked by the bot, so members never grant access to their server list.

## 3. Bot

1. **Bot → Reset Token**: copy the token. This is `DISCORD_BOT_TOKEN`, an environment variable only.
2. **Privileged Gateway Intents**: leave all three off. The app reads members by REST (`GET /guilds/{guild}/members/{user}`), which needs no intent.
3. **Public Bot**: turn it off, so only you can add the bot to a server.

The bot must be in the server: sign-in uses the bot token to check that the user is a member, and the admin check reads the server's roles.

## 4. Activities

1. **Activities → Settings**: turn on **Enable Activities**. Discord then creates the default *Entry Point* command ("Launch"), a global command that opens the Activity from the App Launcher. Leave it as it is. The bot registers its own commands per server. On start it changes one thing on this global command: its handler, so the bot launches the Activity itself and Discord doesn't post "… used Launch" in the channel on every launch ([ADR 0019](adr/0019-silent-entry-point.md)). Opening the Activity from the App Launcher therefore needs the bot to be running.
2. **Activities → Settings → Supported Platforms**: tick **Web**, **iOS** and **Android**. An unticked platform doesn't show the Activity at all, and members use the Discord mobile apps too.
3. **Activities → URL Mappings**: add **Prefix** `/` and **Target** your host *without* `https://`:
   - production: your public hostname, e.g. `pickem.example.com` (served on the Pi through the bundled Caddy or your Traefik; see [deploy/README.md](../deploy/README.md));
   - development: the cloudflared tunnel host, e.g. `funky-jogging-bunny.trycloudflare.com` (see the README).

No other mappings are needed. The Activity calls only its own origin: the API under `/api`, and avatars and team logos through the backend's image proxy (`/api/img/...`).

## 5. Install to your server

1. **Installation → Installation Contexts**: tick **Guild Install**. *User Install* is not needed: the app only works inside a server, and in DMs it only says "Open this from the server".
2. **Installation → Default Install Settings → Guild Install**: scopes `applications.commands` and `bot`. Bot permissions (for the channel posts and commands): *Send Messages*, *Embed Links*, *Use Application Commands*.
3. Open the **Install Link** (`https://discord.com/oauth2/authorize?client_id=<Application ID>`) and add the app to your server. This needs *Manage Server* there.

## 6. Configure the app

| Portal value | Environment variable |
| --- | --- |
| General Information → Application ID | `DISCORD_CLIENT_ID` |
| OAuth2 → Client Secret | `DISCORD_CLIENT_SECRET` |
| Bot → Token | `DISCORD_BOT_TOKEN` |
| Your server's ID (Discord: *User Settings → Advanced → Developer Mode*, then right-click the server → *Copy Server ID*) | `DISCORD_GUILD_ID` (recommended) |

With `DISCORD_GUILD_ID` set, the app accepts only that server. Without it, it accepts any server the bot is in, checking membership each time. The frontend reads the client ID from the backend (`GET /api/auth/config`), so one build serves any application.

## 7. Launch it

In the Discord client, open a voice or text channel of the server, open the **App Launcher** (the rocket icon) and pick the Activity. While developing, keep **Application URL Override** off when you use a tunnel.

The bot's posts and replies also carry buttons that open the Activity. *Pick* and *View* buttons open it on that match: the bot remembers the match for 10 minutes and the Activity opens it after signing in, because Discord passes no launch parameters.

## 8. Bot commands and posts

Once the app runs, the bot registers its slash commands in the server (in `DISCORD_GUILD_ID`, else in every server it is in). They show up at once.

- Members: `/leaderboard` (top 10) and `/mypicks` (your open picks). Both answer only to you, with a button that opens the Activity.
- Admins, under `/admin`:
  - `channel set` picks the channel for posts;
  - `role set` sets the pick'em admin role;
  - `digest set` sets the digest's time and time zone;
  - `status` shows sync health and API quota;
  - `link list | merge | create` handles source entities that could not be matched;
  - `match result | clear-override | void | unvoid | reopen` fixes matches by hand.

  Everyone sees the `/admin` commands, but only admins can run them; others get a private error.
- Admins, under `/event`, choose which events are synced and can be picked:
  - `search <name>` asks every data source that can search (at most one request per source, then cached for 24 h) and follows the event you choose in the menu. Its matches show up in the Activity within a minute;
  - `follow <event>` follows an event the app already knows, for example one you unfollowed before;
  - `unfollow <event>` stops the sync and hides its matches. Picks, points and history stay. When members have picks on it, the bot asks first;
  - `list` shows the followed events with their match and pick counts.

  Only matches of followed events show up in the lists and posts. Like `/admin`, only admins can run them.

Set up the posts:

1. Run `/admin channel set` in the server. Results of matches that finished before this are not posted.
2. Make sure the bot has *Send Messages* and *Embed Links* in that channel. Channel overrides can take them away.
3. Optionally run `/admin digest set 10:00 Europe/Berlin` (these are the defaults).

The bot then posts:

- the **daily digest** at that local time, on days with a match still open for picks. It lists the day's matches with lock times and pick counts, and is edited as picks come in;
- **results** 10 minutes after a match finishes. Matches that finish within those 10 minutes share one post, with the final and map scores and the best pickers.

At most 6 posts go out per hour. Each post is made once, also across restarts.

## Admins

Members with *Manage Server* in the server (including *Administrator* and the owner) are admins of the pick'em. A role can be added with `/admin role set`. The Activity reads admin status from Discord and caches it for 5 minutes. Bot commands check it on every command from what Discord sends with it.

## Who can launch it

Discord limits Activities that are not public yet:

- Discord's docs: "Launching a non-distributed Activity is limited to you or members of the developer team."
- Discord's developer support articles also say unverified Activities are visible only to the owner, the developer team and up to 50 **App Testers** (*App Testers* in the portal), and can only be launched in servers with **fewer than 25 members**.

For a larger server, check Discord's current rules for making the Activity available to all members (app verification). This is an open point for the project, see [vision.md](vision.md#open-questions).

## Sources

- [Building an Activity](https://docs.discord.com/developers/activities/building-an-activity): portal setup, placeholder redirect, Entry Point command, team access
- [Local development](https://docs.discord.com/developers/activities/development-guides/local-development): URL mappings, supported platforms, Application URL Override
- [Networking](https://docs.discord.com/developers/activities/development-guides/networking): the proxy and CSP
- [OAuth2](https://docs.discord.com/developers/topics/oauth2): code exchange
- [Guild resources](https://docs.discord.com/developers/resources/guild) and [Permissions](https://docs.discord.com/developers/topics/permissions): membership and *Manage Server*
- [Receiving and responding](https://docs.discord.com/developers/interactions/receiving-and-responding) and [How Activities work](https://docs.discord.com/developers/activities/how-activities-work): `LAUNCH_ACTIVITY` from commands and buttons
- [Application commands](https://docs.discord.com/developers/interactions/application-commands): the Entry Point command is global only
- [Messages](https://docs.discord.com/developers/resources/message): `nonce` and `enforce_nonce`; [Message formatting](https://docs.discord.com/developers/reference): `<t:…>` timestamps
- Discord developer support: [How can my friends play my Activity](https://support-dev.discord.com/hc/articles/21204493235991-How-Can-My-Friends-Play-My-Activity), [Private Activity safety](https://support-dev.discord.com/hc/articles/21692628851351-Private-Activity-Safety)
