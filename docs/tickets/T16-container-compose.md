# T16 – Container image and Raspberry Pi compose stack

- **Epic:** E – Deployment and CI/CD
- **Status:** todo
- **Index:** [all tickets](README.md)

**Goal:** `docker compose up -d` on a Raspberry Pi runs the whole product.

**Depends on:** T01 (can start early; finish after T07)

**Scope**

- Multi-stage `Dockerfile`: a Node stage builds the frontend, a Rust stage builds the backend, and the final stage is `debian:bookworm-slim` (or distroless `cc`) with the binary, `dist/`, CA certificates and a non-root user.
- `app healthcheck` subcommand used by the Docker `HEALTHCHECK`, so the image needs no curl.
- `deploy/docker-compose.yml`:
  - `app`: image `ghcr.io/<owner>/cs2-pickem:${IMAGE_TAG:-latest}`, `env_file`, volume `./data:/data` (SQLite and image cache), `restart: unless-stopped`, `stop_grace_period: 20s`, Watchtower opt-in label, memory limit 256 MB. **No published ports**: reachable only on the internal compose network.
  - `caddy`: `caddy:2` with a `Caddyfile` of `{$DOMAIN} { reverse_proxy app:8080 }`. Publishes 80 and 443 (plus 443/udp for HTTP/3). Gets Let's Encrypt certificates automatically and keeps them in a `caddy_data` volume. Adds security headers and an access log.
  - `watchtower`: see T17.
- **Network setup** (README): a DNS A record, plus AAAA if you use IPv6, for e.g. `pickem.<your-domain>` pointing to your home IP. The router forwards TCP 80 and 443 to the Pi. Use DynDNS (router feature or a `ddclient` container) if the home IP changes. The Discord URL mapping `/` points to that hostname.
- **Hardening:** only Caddy is reachable from outside. The app rate-limits `/api/auth/*` and admin endpoints (e.g. `tower_governor`). The host firewall (ufw) allows 80/443 from anywhere and SSH from the LAN only. Unattended security updates are enabled on the Pi OS.
- `deploy/README.md`: Pi prerequisites (64-bit OS, Pi 4 with 2 GB or a Pi 5, SSD recommended over SD card because of database writes), network setup, first start, logs, update, rollback by pinning `IMAGE_TAG=sha-<short>`.

**Acceptance criteria**

- [ ] On a clean Raspberry Pi OS (64-bit), following the README brings the Activity up through Discord.
- [ ] The container reports *healthy*, and data survives `docker compose down && up`.
- [ ] Final image under 150 MB; idle memory of `app` under 150 MB.

- [ ] `https://<your-domain>/healthz` answers from outside the home network with a valid certificate, and port 8080 is not reachable from outside.
