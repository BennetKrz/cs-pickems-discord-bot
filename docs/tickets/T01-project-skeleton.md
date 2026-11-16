# T01 – Project skeleton, configuration and local dev loop

- **Epic:** A – Foundation and data
- **Status:** todo
- **Index:** [all tickets](README.md)

**Goal:** both halves build and run, and a developer can iterate locally.

**Depends on:** –

**Scope**

- Cargo workspace with crates: `domain` (types, pick rules, scoring; no I/O), `store` (sqlx repositories), `sources` (adapter trait and adapters), `sync` (scheduler, reconciler), `api` (axum), `bot` (poise), `app` (binary that wires everything into one tokio runtime).
- Vite + React + TypeScript app in `frontend/` with a placeholder page. Use an in-memory router: the Activity always loads at `/`.
- The backend serves `frontend/dist` at `/`, the JSON API under `/api/*`, and `/healthz`.
- Typed config from env plus optional `config.toml`: `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET`, `DISCORD_BOT_TOKEN`, `DISCORD_GUILD_ID`, `DATABASE_URL`, `PANDASCORE_TOKEN`, `ENABLED_SOURCES`, `LOG_FORMAT`, `RUST_LOG`, plus budget settings from T06.
- `.env.example`, README with the dev setup. Dev runs `cargo run` plus `pnpm dev`, with Vite proxying `/api`. Discord testing uses a `cloudflared` quick tunnel as in Discord's local-development guide (dev only, no account needed), or a dev subdomain on your own domain.
- Graceful shutdown on SIGTERM: stop the sync worker, close the gateway, drain HTTP, close the DB.

**Acceptance criteria**

- [ ] `cargo run` starts, and `GET /healthz` returns 200 with `{version, git_sha, db: "ok"}`.
- [ ] A missing required env var exits with code 1 and names the variable.
- [ ] SIGTERM ends the process within 10 s without errors in the log.
- [ ] `pnpm build` output is served by the backend at `/`.

**Out of scope:** Docker (T16), CI (T17).
