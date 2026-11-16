# 0002 – Configuration, runtime skeleton and toolchain

- **Status:** accepted
- **Date:** 2026-11-16
- **Ticket:** T01

## Context

T01 sets up the workspace, typed config, graceful shutdown and the local dev loop. Several choices are not obvious from the code:

- The config must name the missing environment variable on error, and secrets must come from the environment only (AGENTS.md).
- Bot commands only arrive in T08, but the shutdown order already includes "close the gateway".
- The dev machine runs NixOS without Node or pnpm.
- Some current major versions are newer than what the ecosystem supports.

## Decision

1. **Config loader written by hand on top of the `toml` crate, not figment or `config`.**
   - Sources, in order: environment, then an optional `config.toml` (`CONFIG_FILE`, default `./config.toml`), then defaults.
   - `.env` is loaded by `dotenvy` and never overrides real variables.
   - Reasons: figment parses numeric env values (Discord snowflakes) as integers, and its missing-field errors name struct fields, not env var names. A pure `from_sources(env, file)` function is easy to test. It collects every problem and reports them together.
2. **Secrets are environment-only.**
   - `DISCORD_CLIENT_SECRET`, `DISCORD_BOT_TOKEN` and `PANDASCORE_TOKEN` in the TOML file are a startup error.
   - Secret values use a `Secret` type with a redacted `Debug` and no `Display`.
   - `PANDASCORE_TOKEN` is required only while `pandascore` is in `ENABLED_SOURCES`.
   - `DISCORD_GUILD_ID` is optional, as in T07.
3. **Budget settings** use `[budget.<source>]` in TOML, overridable as `BUDGET__<SOURCE>__PER_HOUR` / `__PER_MINUTE`. The default is PandaScore at 500/h and 30/min, which is 50% of the free plan (T06).
4. **Stub bot until T08.** The `bot` crate's task only waits for cancellation; there is no gateway connection and no poise dependency yet. The Discord credentials are still validated at startup, so the config contract does not change later.
5. **Shutdown order.**
   - Each component has its own `CancellationToken`. On SIGTERM or SIGINT they stop in order: sync worker, then bot, then HTTP (axum graceful shutdown drains requests), then `pool.close()`.
   - The whole sequence is capped at 8 s. That is inside the 10 s acceptance criterion and Docker's default grace period.
6. **`DATABASE_URL` must use the `sqlite:` scheme.** sqlx 0.9 treats an unparsable URL such as `postgres://…` as a file name and silently creates a stray file.
7. **Static files without SPA fallback.** The Activity uses an in-memory router and always loads at `/`, so a missing asset is a real 404. Unknown `/api/*` paths return a JSON 404.
8. **Workspace layout:** `backend/Cargo.toml` with crates in `backend/crates/*`. The `app` crate is a library plus a thin `main.rs`, so its config types form a public API for later tickets and integration tests.
9. **Toolchain.**
   - A `flake.nix` devShell (with direnv `.envrc`) provides Rust, Node 22, pnpm, SQLite and cloudflared.
   - TypeScript is pinned to `~6.0`, because typescript-eslint 8.x supports `<6.1` and TypeScript 7 (the native port) has no JS compiler API for it yet.
   - `pnpm dev:tunnel` (Vite mode `tunnel`) sets `server.ws.clientPort = 443` for HMR through a cloudflared quick tunnel. Plain `pnpm dev` stays on localhost.

## Consequences

- T08 replaces `bot::run` with a poise framework and closes the shard manager on cancel. Its signature (`CancellationToken`) stays the same.
- T16's Docker image must set `STATIC_DIR` (the default `../frontend/dist` is for dev) and may pass `GIT_SHA` as a build arg, because `.git` is absent in the build context.
- Revisit the TypeScript pin when typescript-eslint supports TypeScript 7.
