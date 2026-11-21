# 0008 – Container image, optional Caddy or existing Traefik, rate limits

- **Status:** accepted
- **Date:** 2026-11-18
- **Ticket:** T16

## Context

T16 packages the app for a Raspberry Pi (arm64): an image under 150 MB, a compose stack, TLS on the owner's domain, and rate limits on `/api/auth/*` and the admin endpoints. Planning ([ADR 0000](0000-initial-decisions.md), point 7) chose Caddy for TLS. The owner already runs Traefik on the Pi, so the bundled Caddy must be optional, with a ready-made way into an existing Traefik.

Facts that shaped the details (checked 18 Nov 2026):

- Discord's Activity proxy runs on Cloudflare Workers and exists partly "to hide the users' IP addresses". All Activity traffic reaches us from Discord's proxy.
- `aws-lc-rs` (rustls) and the bundled SQLite compile C code, so an arm64 build needs a C cross toolchain ([ADR 0001](0001-pandascore-coverage.md)).
- Watchtower's nickfedor fork still uses the label `com.centurylinklabs.watchtower.enable`.
- Traefik is at v3 (docs v3.7). The headers middleware options are `stsSeconds`, `contentTypeNosniff`, `referrerPolicy` and `customResponseHeaders`.

## Decision

1. **Image.**
   - A multi-stage `Dockerfile` at the repo root. The frontend (`node:22-slim`, pnpm) and backend (`rust:1-slim-trixie`) stages run on `$BUILDPLATFORM`.
   - The Rust stage cross-compiles to `$TARGETARCH` with Debian's `gcc-<arch>-linux-gnu`. The final stage only copies files, so an arm64 image builds on amd64 without QEMU, and T17 can reuse the Dockerfile as is.
   - The final image is `gcr.io/distroless/cc-debian13:nonroot`, with glibc, CA certificates and user 65532 but no shell. It is 68 MB on disk (18 MB compressed).
   - The image sets `STATIC_DIR`, `DATABASE_URL=sqlite:///data/pickem.db`, `IMAGE_CACHE_DIR=/data/img-cache`, `BIND_ADDR`, `LOG_FORMAT=json`; `GIT_SHA` is a build argument.
   - The release profile uses thin LTO and strips symbols.
2. **`app healthcheck`** probes `GET /healthz` on `BIND_ADDR` (a wildcard address becomes loopback) over a plain `TcpStream`, before any config is loaded. It needs no curl, no shell and no secrets, and adds no dependencies.
3. **The reverse proxy is a choice in `deploy/.env`.**
   - `caddy` is in the compose profile `caddy`. The example `.env` sets `COMPOSE_PROFILES=caddy`, so `docker compose up -d` works out of the box.
   - For Traefik: `COMPOSE_PROFILES=` plus `COMPOSE_FILE=docker-compose.yml:docker-compose.traefik.yml`. That override only attaches `app` to Traefik's external network.
   - The route is a separate choice, so a Traefik that also watches Docker never gets a second, label-made router next to a hand-written one:
     - `docker-compose.traefik-labels.yml` (appended to `COMPOSE_FILE`) adds the router, middleware and service labels for the Docker provider;
     - or `deploy/traefik/pickem.yml` is the same router for the owner's own Traefik config (file provider). It reaches the container by its fixed name `pickem-app`.
   - `app` publishes no port in either mode.
4. **The security headers live in the proxy** (HSTS without subdomains, `nosniff`, `Referrer-Policy`, no `Server`). They are written twice, in the Caddyfile and in the Traefik label/file config, with comments to keep them in sync.
   - No `X-Frame-Options` or `frame-ancestors`, because Discord frames the Activity.
   - No compression at the proxy, because it would alter the API's ETags (ADR 0007, point 7).
5. **Rate limits in the app** use `tower_governor` 0.8 with a custom key: the **rightmost** `X-Forwarded-For` entry, else the TCP peer.
   - Caddy and Traefik (default `forwardedHeaders`) write or append that entry themselves, so a client can't choose its key. The leftmost entry, which the library's `SmartIpKeyExtractor` uses, is client-controlled.
   - Because Discord hides user IPs, Activity users share a key, so the bursts are generous: `/api/auth/*` allows 30 at once, then 1 per 2 s; `/api/admin/*` allows 60, then 1 per s. They mainly stop direct hammering of the public hostname.
   - A 429 uses the API's JSON error shape (`rate_limited`) with `Retry-After`. Idle keys are purged every minute.
6. **Data directory ownership:** compose runs the app as `APP_UID:APP_GID` (default 1000), so `./data` belongs to the Pi user and is easy to back up. The image's own `/data` belongs to 65532 for runs without a volume.

## Consequences

- This amends ADR 0000, point 7: TLS is terminated by the bundled Caddy *or* an existing Traefik. The rest of the hosting decision (own domain, ports 80/443, no Cloudflare Tunnel) stands.
- The `/api/auth/*` rate limit left open in ADR 0007 is done.
- If the app is ever exposed without a proxy, `X-Forwarded-For` becomes spoofable and the rate limit can be bypassed. Never publish port 8080.
- A Traefik with `forwardedHeaders.trustedIPs` or `insecure` passes the client's header on, but it still appends the address it saw, so the rightmost entry stays correct.
- The Traefik router name `cs2-pickem` is fixed (compose doesn't interpolate label keys). Rename it in `docker-compose.traefik-labels.yml` if it clashes.
- The amd64 build (T17 builds both) also cross-compiles from an arm64 builder; the Dockerfile handles both directions.
- distroless has no shell: debug with `docker compose logs`, or `docker run --rm --entrypoint /app/app … --help`. P4's `app restore` runs as a subcommand, as planned.
