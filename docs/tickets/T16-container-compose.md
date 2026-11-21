# T16 – Container image and Raspberry Pi compose stack

- **Epic:** E – Deployment and CI/CD
- **Status:** in review
- **Index:** [all tickets](README.md)

**Goal:** `docker compose up -d` on a Raspberry Pi runs the whole product, behind either the bundled Caddy or an existing Traefik.

**Depends on:** T01 (can start early; finish after T07)

**Scope**

- Multi-stage `Dockerfile`: a Node stage builds the frontend, a Rust stage builds the backend, and the final stage is `debian:bookworm-slim` (or distroless `cc`) with the binary, `dist/`, CA certificates and a non-root user.
- `app healthcheck` subcommand used by the Docker `HEALTHCHECK`, so the image needs no curl.
- `deploy/docker-compose.yml`:
  - `app`: image `ghcr.io/<owner>/cs2-pickem:${IMAGE_TAG:-latest}`, `env_file`, volume `./data:/data` (SQLite and image cache), `restart: unless-stopped`, `stop_grace_period: 20s`, Watchtower opt-in label, memory limit 256 MB. **No published ports**: reachable only through the reverse proxy.
  - `caddy` (**optional**, compose profile `caddy`, enabled by default in the example `.env`): `caddy:2` with a `Caddyfile` of `{$DOMAIN} { reverse_proxy app:8080 }`. Publishes 80 and 443 (plus 443/udp for HTTP/3). Gets Let's Encrypt certificates automatically and keeps them in a `caddy_data` volume. Adds security headers and an access log.
  - `watchtower`: see T17.
- **Existing Traefik instead of Caddy:** with Caddy's profile disabled, the override `deploy/docker-compose.traefik.yml` attaches `app` to Traefik's external Docker network. It carries router, TLS (certresolver), security-header middleware and service labels, configurable through `.env` (network, entrypoint, resolver). `deploy/traefik/pickem.yml` is the same configuration as a snippet for Traefik's file provider.
- **Network setup** (README): a DNS A record, plus AAAA if you use IPv6, for e.g. `pickem.<your-domain>` pointing to your home IP. The router forwards TCP 80 and 443 to the Pi (to Caddy, or to your Traefik). Use DynDNS (router feature or a `ddclient` container) if the home IP changes. The Discord URL mapping `/` points to that hostname.
- **Hardening:** only the reverse proxy (Caddy or Traefik) is reachable from outside. The app rate-limits `/api/auth/*` and admin endpoints (e.g. `tower_governor`). The host firewall (ufw) allows 80/443 from anywhere and SSH from the LAN only. Unattended security updates are enabled on the Pi OS.
- `deploy/README.md`: Pi prerequisites (64-bit OS, Pi 4 with 2 GB or a Pi 5, SSD recommended over SD card because of database writes), network setup, both proxy options, first start, logs, update, rollback by pinning `IMAGE_TAG=sha-<short>`.

**Acceptance criteria**

- [ ] On a clean Raspberry Pi OS (64-bit), following the README brings the Activity up through Discord.
- [x] The container reports *healthy*, and data survives `docker compose down && up`.
- [x] Final image under 150 MB; idle memory of `app` under 150 MB.
- [ ] `https://<your-domain>/healthz` answers from outside the home network with a valid certificate, and port 8080 is not reachable from outside.
- [x] With Caddy's profile disabled and the Traefik override enabled, an existing Traefik serves the app (labels or file-provider snippet), and no Caddy container runs.

**Notes**

- Decisions: [ADR 0008](../adr/0008-container-image-and-proxy.md). The final image is distroless `cc-debian13:nonroot`. The Rust stage cross-compiles with Debian's cross gcc, so an arm64 image builds on amd64 without QEMU.
- Proxy choice in `deploy/.env`: `COMPOSE_PROFILES=caddy` (default), or `COMPOSE_PROFILES=` with `COMPOSE_FILE=docker-compose.yml:docker-compose.traefik.yml`.
- Rate limits (`api::ratelimit`): `/api/auth/*` allows 30 at once, then 1 per 2 s; `/api/admin/*` allows 60, then 1 per s. The key is the rightmost `X-Forwarded-For` entry, else the peer. The burst is generous because Discord's proxy hides user IPs.
- Tests:
  - `api/src/ratelimit.rs`: key extraction, the JSON 429 with `Retry-After`, separate buckets, no limit on health and frontend;
  - `app/src/healthcheck.rs`: target resolution and the 200/503/garbage/refused cases;
  - `app/tests/process.rs`: `app healthcheck` against the real binary, and without a server.
- Checked locally on 18 Nov 2026 (amd64, Docker 29.9):
  - the image is 68 MB on disk (18 MB compressed);
  - `app` reports *healthy* and idles at about 3 MiB of memory;
  - data survives `down && up`, and `stop` shuts down gracefully;
  - Caddy (`DOMAIN=localhost`, internal CA) and a throwaway `traefik:v3` (B1 labels and B2 file snippet) both serve `/healthz` with the security headers;
  - port 8080 isn't reachable from the host;
  - through both proxies, 30 auth requests pass, then a 429 follows, and a forged `X-Forwarded-For` doesn't help;
  - the arm64 image cross-builds (aarch64 ELF, 18 MB), but couldn't be run here (no QEMU).
- Open for the owner, on the Pi: AC1 (clean Pi OS, Activity through Discord) and AC4 (reachable from outside with a real certificate).
- Follow-ups for T17:
  - build with `docker buildx build --platform linux/arm64,linux/amd64 --build-arg GIT_SHA=…`, using cache mounts or a GHA cache;
  - Watchtower must use label-enable mode, so it never touches Caddy or the user's Traefik;
  - consider adding the `org.opencontainers.image.source` label, so GHCR links the package to the repo.
