# CS2 Pick'em – agent instructions

Self-hosted Discord app for one server's Counter-Strike 2 pick'em: members predict match winners, series scores and map scores, and points feed a server leaderboard. A Discord Activity (embedded web app) holds all screens; a bot posts announcements. Runs with docker compose on a Raspberry Pi.

## Where things are

- Product goal, decisions, glossary: `docs/vision.md`
- Architecture, data flow, invariants: `docs/architecture.md`
- Tickets, build order and status: `docs/tickets/README.md`, one file per ticket in `docs/tickets/`
- Decision records: `docs/adr/`
- Code: `backend/` (Cargo workspace: `domain`, `store`, `sources`, `sync`, `api`, `bot`, `backup`, `app`), `frontend/` (React + Vite + TS), `deploy/` (compose, Caddyfile, Traefik overrides (network, labels) and snippet, env example; `Dockerfile` at the repo root)

Read `docs/vision.md` and `docs/architecture.md` before your first change in a session, and the ticket you are working on before you start it.

## Commands

- Backend checks: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- Frontend checks: `pnpm lint && pnpm typecheck && pnpm test`
- Run locally: `cargo run` (backend) and `pnpm dev` (frontend, proxies `/api`)

All checks must pass before you call a ticket done.

## Workflow

- Work on exactly one ticket at a time, on its own branch (`ticket/T03-source-adapters`), ending in one PR.
- Follow the build order in `docs/tickets/README.md`. Don't start a ticket whose dependencies are not done.
- A ticket is done when every acceptance criterion is met and checks pass. Then tick the criteria in the ticket file and set its status in the ticket file and the index.
- Stay inside the ticket's scope. If you find missing or conflicting requirements, stop and ask instead of guessing. Note follow-ups in the ticket's *Notes*.
- Record every non-obvious decision (spike results, library choices, changed defaults) as an ADR in `docs/adr/`.
- Never call real external APIs in tests. Use recorded fixtures and the `fixture` source (T05).

## Cross-cutting rules

- All times are UTC (RFC 3339) in storage and APIs; only the frontend converts to local time.
- Only the sync worker calls external data APIs. Bot and API read the own database.
- Secrets come from environment variables only and are never logged or committed.
- Every identity and guild claim from the browser is re-verified on the server.
- The UI says "pick", never "bet"; no money or odds anywhere.
- Before relying on Discord, PandaScore or Watchtower API details, check their current official docs. The tickets may be outdated.
