# Running CS2 Pick'em on a Raspberry Pi

One container (`app`: bot, API, sync worker and the Activity) runs behind a reverse proxy that terminates TLS for your domain. Pick one:

- **Option A: the bundled Caddy** (default). It publishes ports 80/443 and gets Let's Encrypt certificates by itself.
- **Option B: an existing Traefik v3** that already runs on the Pi. No Caddy container starts. `app` joins Traefik's Docker network, and you get either labels (B1) or a file-provider snippet (B2).

In both cases `app` publishes no port. Discord's proxy reaches the Activity through `https://<your domain>/`.

| File | Purpose |
| --- | --- |
| `docker-compose.yml` | `app`, plus `caddy` in the compose profile `caddy` |
| `Caddyfile` | Option A: TLS, security headers, access log |
| `docker-compose.traefik.yml` | Option B: puts `app` on Traefik's network, with router labels (B1) |
| `traefik/pickem.yml` | Option B2: the same router for Traefik's file provider |
| `.env.example` | Every setting, compose and app, in one file |

## 1. Prerequisites

- A Raspberry Pi 4 (2 GB or more) or a Pi 5, running **Raspberry Pi OS 64-bit** (Lite is enough). The image is arm64 only.
- **An SSD over USB rather than the SD card.** SQLite writes all the time, and SD cards wear out and corrupt.
- Docker Engine with the compose plugin:

  ```sh
  curl -fsSL https://get.docker.com | sh
  sudo usermod -aG docker $USER   # log out and back in
  docker compose version
  ```

- A domain you control, and a home connection that can forward IPv4 ports 80 and 443. DS-Lite and CGNAT connections can't; ask your ISP for a public IPv4 address.
- The Discord application from [docs/discord-setup.md](../docs/discord-setup.md).

## 2. Network

1. **DNS:** create an `A` record (and `AAAA` if you use IPv6) for e.g. `pickem.<your-domain>` that points to your home IP. If the IP changes, use DynDNS: the router's DynDNS feature, or a `ddclient` container.
2. **Router:** forward TCP 80 and 443 (and UDP 443 for HTTP/3 with Caddy) to the Pi. With Option B, Traefik already owns these ports.
3. **Discord:** in the Developer Portal, go to **Activities → URL Mappings** and point `/` to `pickem.<your-domain>` (no `https://`).

## 3. Configure

```sh
git clone https://github.com/<owner>/<repo>.git cs2-pickem
cd cs2-pickem/deploy
cp .env.example .env && chmod 600 .env
mkdir -p data          # SQLite and image cache; must belong to APP_UID
```

Fill in `.env`:

- `DOMAIN`;
- `APP_IMAGE` (`ghcr.io/<owner>/cs2-pickem`, lowercase owner);
- the Discord values and `PANDASCORE_TOKEN`;
- `APP_UID`/`APP_GID`: the output of `id -u` and `id -g`, usually 1000.

If a secret contains `$`, wrap the value in single quotes. If the GHCR package is private, run `docker login ghcr.io` once with a token that has `read:packages`.

### Option A: bundled Caddy

Keep `COMPOSE_PROFILES=caddy`; nothing else is needed. Caddy requests the certificate on first start, and port 80 must be reachable for that. Certificates live in the `caddy_data` volume.

### Option B: existing Traefik

Requirements on your Traefik (v3):

- an HTTPS entrypoint, by default named `websecure`;
- a certificate resolver, by default named `letsencrypt`;
- a Docker network that Traefik is attached to, by default named `traefik`. Create it once with `docker network create traefik` if it doesn't exist yet.

In `.env`:

```sh
COMPOSE_PROFILES=
COMPOSE_FILE=docker-compose.yml:docker-compose.traefik.yml
TRAEFIK_NETWORK=traefik          # your names, if they differ
TRAEFIK_ENTRYPOINT=websecure
TRAEFIK_CERTRESOLVER=letsencrypt
```

With these two lines, plain `docker compose …` commands use the Traefik override and never start Caddy.

- **B1: Docker provider (labels).** Nothing else to do. The labels in `docker-compose.traefik.yml` define the router `cs2-pickem` (``Host(`$DOMAIN`)``, TLS through your resolver), the `cs2-pickem-headers` middleware and the service on port 8080. If Traefik runs with `exposedByDefault=false`, the `traefik.enable=true` label opts in. To add your own middlewares (e.g. CrowdSec), append them to `traefik.http.routers.cs2-pickem.middlewares`.
- **B2: file provider.** Copy `traefik/pickem.yml` into the directory your file provider watches, then replace the host, the entrypoint and the resolver in it. It reaches the app as `http://pickem-app:8080` over the shared network. Without the Docker provider the labels are simply ignored. Use B1 or B2, not both.
- **Traefik on another machine:** this compose stack doesn't cover that case. The app would need a published port, e.g. `ports: ["<pi-lan-ip>:8080:8080"]` in an override file, and the file-provider URL `http://<pi-lan-ip>:8080`. Don't forward 8080 on the router.

Rate limits trust the **last** `X-Forwarded-For` entry, which your proxy writes. Both Caddy and Traefik (with its default `forwardedHeaders` settings) overwrite or append that header correctly. Don't publish port 8080 to the Internet.

## 4. First start

```sh
docker compose pull
docker compose up -d
docker compose ps              # app: "Up … (healthy)"
curl https://pickem.<your-domain>/healthz
# {"version":"…","git_sha":"…","db":"ok"}
```

Then open the Activity from your Discord server.

## 5. Operations

| Task | Command |
| --- | --- |
| Logs (JSON) | `docker compose logs -f app` (Caddy's access log: `docker compose logs -f caddy`) |
| Health | `docker compose ps`, or `docker inspect -f '{{.State.Health.Status}}' pickem-app` |
| Update now | `docker compose pull && docker compose up -d` (Watchtower does this automatically, T17) |
| Roll back | set `IMAGE_TAG=sha-<short>` in `.env`, then `docker compose up -d`. Back to `latest` when fixed. |
| Restart | `docker compose restart app` (graceful, within 8 s) |
| Stop everything | `docker compose down` (the data in `./data` stays) |
| Back up by hand | `docker compose stop app && cp -a data data.bak-$(date +%F) && docker compose start app` (automatic backups: P4) |

The SQLite database is `data/pickem.db` (with `-wal` and `-shm` while running). The image cache is `data/img-cache` and can be deleted at any time.

## 6. Hardening

- Only the reverse proxy is reachable from outside. `app` publishes no port, and the app rate-limits `/api/auth/*` and `/api/admin/*`.
- **Firewall (ufw):** allow 80/443 from anywhere and SSH from the LAN only (adjust the subnet):

  ```sh
  sudo apt install ufw
  sudo ufw default deny incoming
  sudo ufw allow from 192.168.0.0/16 to any port 22 proto tcp
  sudo ufw allow 80/tcp
  sudo ufw allow 443/tcp
  sudo ufw allow 443/udp
  sudo ufw enable
  ```

  Docker writes its own iptables rules, and **published container ports bypass ufw**. Publish only what may be public; this stack publishes only 80/443 (Caddy).
- **Unattended security updates:**

  ```sh
  sudo apt install unattended-upgrades
  sudo dpkg-reconfigure -plow unattended-upgrades
  ```

- Use SSH keys and set `PasswordAuthentication no`.

## 7. Trying it locally (no domain)

Build the image and use Caddy's internal CA for `localhost`:

```sh
docker buildx build --load -t cs2-pickem:dev --build-arg GIT_SHA=$(git rev-parse --short HEAD) .   # repo root
cd deploy && cp .env.example .env && mkdir -p data
# .env: DOMAIN=localhost, APP_IMAGE=cs2-pickem, IMAGE_TAG=dev, APP_UID/GID=$(id -u)/$(id -g),
#       placeholder Discord values, ENABLED_SOURCES=manual
docker compose up -d --wait
curl -k https://localhost/healthz
```

Inside Discord this needs a real domain. To build an arm64 image on an amd64 machine, add `--platform linux/arm64`. The Dockerfile cross-compiles, so no QEMU is needed.
