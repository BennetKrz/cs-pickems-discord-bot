# CS2 Pick'em

A self-hosted Discord app for one server's Counter-Strike 2 pick'em. Members pick match winners, series scores and map scores, and points feed a server leaderboard. All screens live in a Discord **Activity** (an embedded web app); a bot posts announcements. It runs with docker compose on a Raspberry Pi.

- Product goal and decisions: [docs/vision.md](docs/vision.md)
- Architecture: [docs/architecture.md](docs/architecture.md)
- Tickets and build order: [docs/tickets/README.md](docs/tickets/README.md)
- Decision records: [docs/adr/](docs/adr/)

## Layout

| Path | What |
| --- | --- |
| `backend/` | Cargo workspace. Crates in `backend/crates/`: `domain`, `store`, `sources`, `sync`, `api`, `bot`, and `app` (the binary) |
| `frontend/` | The Activity: React + Vite + TypeScript, built to `frontend/dist` |
| `deploy/` | Compose stack, Caddyfile, Traefik overrides (network, labels) and snippet, env example (T16); setup in [deploy/README.md](deploy/README.md). The `Dockerfile` is at the repo root. |

The backend is one binary. It serves the built frontend at `/`, the JSON API under `/api/*`, and `GET /healthz`.

## Development setup

### Toolchain

With Nix (recommended), the flake's devShell provides Rust, Node 22, pnpm, SQLite and cloudflared:

```sh
direnv allow          # or: nix develop
```

Without Nix, install:

- a stable Rust toolchain (edition 2024, Rust ≥ 1.85) with `clippy` and `rustfmt`;
- Node.js ≥ 22 and pnpm (`corepack enable` picks up the pinned version from `frontend/package.json`);
- `cloudflared`, only for testing inside Discord.

### Configuration

```sh
cp .env.example .env
```

The placeholder Discord values are fine for working in a normal browser with a dev build (see [Signing in without Discord](#signing-in-without-discord)). Signing in inside Discord needs a real application: see [docs/discord-setup.md](docs/discord-setup.md). `cargo run` finds `.env` by walking up from `backend/`. Real environment variables always win over `.env`.

| Variable | Required | Default | Notes |
| --- | --- | --- | --- |
| `DISCORD_CLIENT_ID` | yes | – | numeric application ID |
| `DISCORD_CLIENT_SECRET` | yes | – | secret, env only |
| `DISCORD_BOT_TOKEN` | yes | – | secret, env only |
| `DISCORD_GUILD_ID` | no | – | when set, only this server is accepted (recommended) |
| `DATABASE_URL` | yes | – | `sqlite://path/to/file.db`; file and directory are created |
| `PANDASCORE_TOKEN` | if `pandascore` is enabled | – | secret, env only |
| `ENABLED_SOURCES` | no | `pandascore` | comma-separated: `pandascore`, `manual`, `fixture` (`fixture` only in dev builds, see below) |
| `LOG_FORMAT` | no | `pretty` | `pretty` or `json` |
| `RUST_LOG` | no | `info` | [`tracing` filter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) |
| `BIND_ADDR` | no | `0.0.0.0:8080` | |
| `STATIC_DIR` | no | `../frontend/dist` | built frontend |
| `IMAGE_CACHE_DIR` | no | `data/img-cache` | disk cache of the image proxy for avatars and team logos (T07) |
| `BUDGET__<SOURCE>__PER_HOUR` / `__PER_MINUTE` | no | PandaScore 500 / 30 | request budget for the sync worker (T06) |
| `SYNC__<KEY>` | no | see below | sync cadences such as `SYNC__LIVE_EVERY=2m` (T06) |
| `CONFIG_FILE` | no | `./config.toml` if present | optional TOML file |

Non-secret settings can also go in a TOML file; see [config.example.toml](config.example.toml). The precedence is environment, then file, then default. Secrets in the file are rejected at startup. When something is missing or invalid, the app exits with code 1 and lists every problem, naming the variable to set.

### Sync and API budget

Only the sync worker calls external APIs, and only for events an admin follows. Each job is one batched call for all its matches:

| Data | Default cadence | Setting |
| --- | --- | --- |
| Followed event structure (stages, bracket, match list) | every 6 h, and right after following or a resync | `structure_every` |
| Matches starting in the next 48 h | every 30 min | `upcoming_window`, `upcoming_every` |
| Matches starting within 1 h (or up to 6 h overdue) | every 5 min | `imminent_window`, `imminent_every`, `overdue_window` |
| Live matches | every 2 min | `live_every` |
| Finished matches | at +10 min and +2 h, then never | `corrections_after` |
| Event search | on demand, cached 24 h | `search_cache` |
| Lock ticker (no API calls) | every 30 s | `lock_every` |

Set them in `[sync]` in the config file or as `SYNC__<KEY>`, with durations like `30s`, `5m`, `6h`, `2d`. The budget per source (default 500/h and 30/min for PandaScore, half the free plan) is a pair of token buckets. When it runs short, live polling goes first and the lower priorities wait. Counters and job states are at `GET /api/admin/status`, and `POST /api/admin/events/{id}/resync` forces a structure sync. Details are in [ADR 0006](docs/adr/0006-sync-scheduler-budget.md).

Expected PandaScore requests, measured by the simulation in `backend/crates/sync/tests/scheduler.rs`:

| Day | Followed events | Matches | Most live at once | Requests in 24 h | Peak hour | Peak minute |
| --- | --- | --- | --- | --- | --- | --- |
| Typical | 1 | 6 | 2 | ~305 | 37 | 4 |
| Busy | 3 | 20 | 4 | ~816 | 60 | 13 |

Both stay far below the budget of 500 per hour.

### Run

Use two terminals:

```sh
cd backend && cargo run            # API on http://localhost:8080
cd frontend && pnpm install && pnpm dev   # http://localhost:5173, proxies /api and /healthz to :8080
```

To check the backend:

```sh
curl localhost:8080/healthz        # {"version":"0.1.0","git_sha":"…","db":"ok"}
```

To serve the production build from the backend, run `pnpm build` in `frontend/` and open http://localhost:8080/.

#### Fixture data and the dev clock

To get realistic data without a PandaScore token, run a **dev build** with the fixture source:

```sh
cd backend && ENABLED_SOURCES=fixture cargo run --features dev
```

This loads one event with a Swiss stage and a double-elimination playoff (Bo1/Bo3/Bo5, TBD slots, a canceled match, overtime maps). The first match starts 30 minutes after the first load. The clock is frozen and only moves when you advance it. Each move syncs the scenario and emits the normal domain events:

```sh
curl localhost:8080/api/dev/clock                                  # {"now":…,"anchor":…}
curl -X POST localhost:8080/api/dev/clock/advance \
     -H 'content-type: application/json' -d '{"minutes": 60}'
```

The anchor and the clock are stored in the database, so a restart resumes where you left off. To start over, delete the dev database.

The fixture event is loaded but not followed, so the lists stay empty until an admin follows it. With a bot token, run `/event search` in Discord with a word from its name. The fixture source answers searches only, because the dev clock drives its sync. Without Discord, insert a row into `followed_events`.

Dev builds also accept admin requests with an `X-Dev-User: <Discord user ID>` header, e.g. a manual score:

```sh
curl -X PUT localhost:8080/api/admin/matches/1/overrides/series \
     -H 'content-type: application/json' -H 'X-Dev-User: 300000000000000001' \
     -d '{"score": [2, 1]}'
```

#### Signing in without Discord

The Activity normally signs in through Discord, which only works inside the Discord client. For a normal browser, dev builds offer a dev sign-in: `POST /api/dev/session {"user_id": "…"}` creates a real session for any Discord user ID, without Discord. `pnpm dev` uses it automatically when the page is not inside Discord, as the user in `VITE_DEV_USER_ID` (default `200000000000000001`):

```sh
cd backend && ENABLED_SOURCES=fixture cargo run --features dev
cd frontend && VITE_DEV_USER_ID=300000000000000001 pnpm dev   # open http://localhost:5173
```

API calls by hand need the session as a bearer token:

```sh
TOKEN=$(curl -s -X POST localhost:8080/api/dev/session \
     -H 'content-type: application/json' -d '{"user_id": "300000000000000001"}' | sed -E 's/.*"session_token":"([0-9a-f]+)".*/\1/')
curl -H "Authorization: Bearer $TOKEN" localhost:8080/api/me
```

Admin status in `/api/me` comes from Discord, so with placeholder credentials it is `false` (the log says once that Discord rejected the bot token). Admin routes still take `X-Dev-User`.

Production builds (no `dev` feature) contain none of this:
- `/api/dev/*` is a 404;
- admin requests need a Discord session of a server admin (T07); `X-Dev-User` is ignored;
- `ENABLED_SOURCES=fixture` stops the app at startup;
- outside Discord, the Activity only says "Open this from the server".

To lint and test the dev wiring too, run `cargo clippy --all-targets --features app/dev -- -D warnings && cargo test --features app/dev`.

`Ctrl-C` or `SIGTERM` stops the app gracefully. It stops the sync worker, closes the bot gateway, drains HTTP and closes the database, all within 8 s.

### Testing inside Discord

Activities load through Discord's proxy, so the dev server needs a public HTTPS URL. Following [Discord's local-development guide](https://docs.discord.com/developers/activities/development-guides/local-development):

1. Create a separate **development** application as described in [docs/discord-setup.md](docs/discord-setup.md), install it to your test server, and put its client ID, client secret and bot token into `.env`. Sign-in checks your membership with the bot token, so the bot must be in the server.
2. Run the backend (`cargo run`) and the frontend in tunnel mode:

   ```sh
   cd frontend && pnpm dev:tunnel      # same as pnpm dev, plus HMR over the tunnel (port 443)
   cloudflared tunnel --url http://localhost:5173
   ```

   This is a cloudflared *quick tunnel*: dev only, no account needed. It prints a host such as `https://funky-jogging-bunny.trycloudflare.com`.
3. In the Developer Portal, go to **Activities → URL Mappings** and set `/` to that host, without `https://`.
4. Launch the Activity from the Discord client with **Application URL Override disabled**. It signs you in with your Discord account and shows your name and avatar. To look for CSP errors, open the Activity in the Discord web client (discord.com/app) and check the browser's developer console.

The quick-tunnel host changes each time you start cloudflared, so update the mapping each time. A stable alternative is a dev subdomain on your own domain that points at your machine.

## Checks

Every ticket must pass these checks before it counts as done:

```sh
cd backend  && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cd frontend && pnpm lint && pnpm typecheck && pnpm test
```

Tests never call real external APIs.

## Deployment

The `Dockerfile` (repo root) builds one image with the backend and the built Activity; `deploy/` runs it with docker compose on a Raspberry Pi, behind either the bundled Caddy or an existing Traefik. Setup, operations and hardening: [deploy/README.md](deploy/README.md). Decisions: [ADR 0008](docs/adr/0008-container-image-and-proxy.md).

The image's health check is `app healthcheck`, which probes `GET /healthz` on `BIND_ADDR` and exits 0 only on 200.
