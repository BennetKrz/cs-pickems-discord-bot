# 0000 – Initial decisions from planning

- **Status:** accepted
- **Date:** 2026-11-16
- **Ticket:** –

## Context

The project owner set the requirements and answered the open design questions before implementation started. This ADR records those answers so later changes are deliberate.

## Decision

1. **UI:** a Discord Activity (embedded web app) holds all screens. The bot handles slash commands, channel posts and launching the Activity.
2. **Match scope:** only events that admins follow are synced and pickable, with no automatic follow by tier.
3. **Pick lock:** picks are editable until the match starts; then every part of a pick locks. No per-map locking.
4. **Stack:** a Rust backend (tokio, axum, poise, sqlx) as one binary, and a TypeScript frontend (React, Vite, Embedded App SDK).
5. **Database:** SQLite in WAL mode via sqlx. PostgreSQL stays possible later.
6. **Visibility:** all picks are visible to all server members, also before the lock.
7. **Hosting:** the Raspberry Pi is reached through the owner's own domain, with ports 80/443 forwarded and Caddy handling TLS. No Cloudflare Tunnel.
8. **Updates:** images go to GHCR, and the Watchtower fork `nickfedor/watchtower` updates them, because the original was archived in Dec 2025.
9. **Wording:** the UI says *pick*, not *bet*. Points only, no money.

## Consequences

- An HTTPS origin reachable by Discord's proxy is mandatory, which needs port forwarding, DNS and certificates (T16).
- Admins must follow events before anything is pickable (T09).
- Live polling is needed only for scores, not for locking picks, which keeps the API budget small (T06).
