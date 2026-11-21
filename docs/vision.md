# Vision and decisions

## Product goal

A self-hosted Discord app for one server's Counter-Strike 2 pick'em. Members predict match winners, series scores and map scores; points feed a server leaderboard. Everything happens inside Discord: a Discord **Activity** (an embedded web app, like the Wordle game) holds all screens, and a bot posts announcements and launches the Activity.

There is no money and there are no odds. "Betting" means predictions for points only. The UI calls this a **pick** to avoid the gambling connotation; code and tickets may say *bet*.

## Core requirements

| Requirement | Ticket(s) |
| --- | --- |
| Bot with the whole UI hosted inside Discord, like Wordle | T07, T08 |
| Accounts are Discord accounts | T07 |
| Pluggable data sources behind adapters; the app sees one single source of truth | T03, T04, T05 |
| Own database, so API keys for data sources are used sparingly | T02, T06 |
| Pick the winner, the series score and map scores; every part optional | T10 |
| Picks can be edited until the match starts | T10, T12 |
| Points and settlement | T11 |
| Overview of upcoming, live and past matches with scores | T12 |
| See everyone's picks, also before the match starts | T10 (API), T12 (UI) |
| Tournament matches shown as a tree | T13 |
| Leaderboard for the server | T14 |
| Own score and own picks | T15 |
| Deployable with docker compose on a Raspberry Pi | T16 |
| CI/CD with automatic deployment via Watchtower | T17 |

Optional extras P1–P7 are listed in [tickets/README.md](tickets/README.md).

## Confirmed decisions

| Topic | Decision | Consequence for implementation |
| --- | --- | --- |
| UI | A Discord **Activity** holds all screens. The bot handles slash commands and channel posts. | Needs a public HTTPS origin: the owner's domain, ports 80/443 forwarded to the Pi, TLS terminated by the bundled Caddy or an existing Traefik (T16, [ADR 0008](adr/0008-container-image-and-proxy.md)). |
| Identity | A Discord account is the only account. No passwords, no sign-up. | OAuth2 through the Embedded App SDK, verified server-side. |
| Match scope | Only events that admins **follow** are synced and can be picked. | Keeps API quota low; brackets exist per followed event. |
| Pick lock | Picks can be created, edited and deleted until the match starts; then every part locks. | No per-map live locking. Live polling is only needed for scores. |
| Backend | **Rust**: one binary running bot, HTTP API and sync worker as tokio tasks. | serenity + poise, axum, sqlx, tokio, reqwest. |
| Frontend | **TypeScript**: React + Vite + `@discord/embedded-app-sdk`. | Built to static files, served by the backend. |
| Database | SQLite in WAL mode via sqlx, on a mounted volume. | One file to back up. sqlx keeps a later switch to PostgreSQL cheap. |
| Pick visibility | All picks are visible to all server members, also before the lock. | P7 offers an optional anti-copy mode. |
| Data access | Only the sync worker calls external data APIs. Bot and UI read the own DB only. | API keys are used in a budgeted, predictable way. |
| Deployment | docker compose on a Raspberry Pi (arm64), images on GHCR, auto-update via Watchtower. | The original `containrrr/watchtower` was archived in Dec 2025; use the maintained fork `nickfedor/watchtower`. |

Further decisions are recorded as ADRs in [adr/](adr/).

## Glossary

- **Match**: a series between two teams in Bo1, Bo3 or Bo5 (the "overall game").
- **Map**: one game inside a match, e.g. map 2 on Inferno, score 13:11.
- **Event**: a tournament such as a Major or IEM. It has one or more **stages** (groups, Swiss, playoffs).
- **Pick**: one member's prediction for one match, with three optional parts: winner, series score, map picks.
- **Lock**: the moment a match starts; after it, picks are read-only.
- **Guild**: the Discord server. All user data is keyed by guild ID, even though one deployment serves one server.
- **Source**: an external data provider behind an adapter (PandaScore, manual entry, fixtures, …).
- **Followed event**: an event an admin selected; only these are synced and pickable.

## Open questions

Defaults apply until the owner decides otherwise.

- [ ] Is the home connection reachable on IPv4 ports 80/443 (no DS-Lite or CGNAT)? Does the IP change, so that DynDNS is needed?
- [ ] SQLite (default) or PostgreSQL?
- [ ] Are the default points in T11 fine?
- [ ] Will the GitHub repo be public or private? This affects free arm64 runners and GHCR pull auth.
- [ ] How many members does the server have? Discord shows an unverified Activity only to the app's team and up to 50 app testers, and only in servers with fewer than 25 members (see [discord-setup.md](discord-setup.md#who-can-launch-it)). A larger server needs the Activity made available through Discord's app verification.
