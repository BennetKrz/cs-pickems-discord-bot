# Running CS2 Pick'em on a Raspberry Pi

One container (`app`: bot, API, sync worker and the Activity) runs behind a reverse proxy that terminates TLS for your domain. Pick one:

- **Option A: the bundled Caddy** (default). It publishes ports 80/443 and gets Let's Encrypt certificates by itself.
- **Option B: an existing Traefik v3** that already runs on the Pi. No Caddy container starts and `app` joins Traefik's Docker network. The route comes either from Docker labels (B1) or from your own Traefik config, such as the file-provider snippet (B2).

In both cases `app` publishes no port. Discord's proxy reaches the Activity through `https://<your domain>/`.

| File | Purpose |
| --- | --- |
| `docker-compose.yml` | `app` and `watchtower` (automatic updates), plus `caddy` in the compose profile `caddy` |
| `Caddyfile` | Option A: TLS, security headers, access log |
| `docker-compose.traefik.yml` | Option B: puts `app` on Traefik's network, nothing else |
| `docker-compose.traefik-labels.yml` | Option B1: router, headers and service as Docker labels |
| `traefik/pickem.yml` | Option B2: the same router for Traefik's file provider |
| `.env.example` | Every setting, compose and app, in one file |

Data lives in `./data`: the SQLite database and its automatic backups (see [Backups and restore](#backups-and-restore)).

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

If a secret contains `$`, wrap the value in single quotes.

**Registry login.** The image on GHCR is private, so the Pi needs a read-only token. On GitHub, create a classic personal access token (**Settings → Developer settings → Personal access tokens → Tokens (classic)**) with only the `read:packages` scope. Then log in once, as the user that runs `docker compose`:

```sh
docker login ghcr.io -u <github-user>   # paste the token as the password
```

This writes `~/.docker/config.json`. `docker compose pull` uses it, and Watchtower gets the same file mounted read-only. If the file is somewhere else (for example because you run compose with `sudo`), set `DOCKER_CONFIG_FILE` in `.env`.

### Option A: bundled Caddy

Keep `COMPOSE_PROFILES=caddy`; nothing else is needed. Caddy requests the certificate on first start, and port 80 must be reachable for that. Certificates live in the `caddy_data` volume.

### Option B: existing Traefik

Your Traefik must be attached to a Docker network, by default named `traefik`. Create it once with `docker network create traefik` if it doesn't exist yet. `docker-compose.traefik.yml` puts `app` on that network, where Traefik reaches it as `http://pickem-app:8080`. Where the route comes from is up to you:

- **B1: Docker provider (labels).** `docker-compose.traefik-labels.yml` adds labels that define the router `cs2-pickem` (``Host(`$DOMAIN`)``, TLS through your resolver), the `cs2-pickem-headers` middleware and the service on port 8080. Traefik needs an HTTPS entrypoint (default `websecure`) and a certificate resolver (default `letsencrypt`). In `.env`:

  ```sh
  COMPOSE_PROFILES=
  COMPOSE_FILE=docker-compose.yml:docker-compose.traefik.yml:docker-compose.traefik-labels.yml
  TRAEFIK_NETWORK=traefik          # your names, if they differ
  TRAEFIK_ENTRYPOINT=websecure
  TRAEFIK_CERTRESOLVER=letsencrypt
  ```

  If Traefik runs with `exposedByDefault=false`, the `traefik.enable=true` label opts in. To add your own middlewares (e.g. CrowdSec), append them to `traefik.http.routers.cs2-pickem.middlewares`.
- **B2: your own Traefik config (file provider).** Copy `traefik/pickem.yml` into the directory your file provider watches, or merge it into your existing dynamic config. Then replace the host, the entrypoint and the resolver in it. The container gets no Traefik labels, so it doesn't matter whether your Traefik also watches Docker. In `.env`:

  ```sh
  COMPOSE_PROFILES=
  COMPOSE_FILE=docker-compose.yml:docker-compose.traefik.yml
  TRAEFIK_NETWORK=proxy            # the network your Traefik is on
  ```

With these lines, plain `docker compose …` commands use the overrides and never start Caddy. Use B1 or B2, not both: together they define two routers for the same host.
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
| Update now | `docker compose pull && docker compose up -d` (Watchtower does this within 5 min anyway, see below) |
| Roll back | set `IMAGE_TAG=sha-<short>` in `.env`, then `docker compose up -d`. This also pauses automatic updates. Back to `latest` when fixed. |
| Pause updates | `docker compose stop watchtower` (`start` resumes), or pin `IMAGE_TAG` as for a rollback |
| Update log | `docker compose logs -f watchtower` |
| Restart | `docker compose restart app` (graceful, within 8 s) |
| Stop everything | `docker compose down` (the data in `./data` stays) |
| Back up now | `/admin backup now` in Discord (daily backups run by themselves, see [Backups and restore](#backups-and-restore)) |
| Restore a backup | see [Backups and restore](#backups-and-restore) |

**Troubleshooting**

| Symptom | Cause and fix |
| --- | --- |
| `(code: 14) unable to open database file` in the app log | The app user can't write to `./data`, often because Docker created it as root. Run `sudo chown -R $(id -u):$(id -g) data` and set `APP_UID`/`APP_GID` to match. Also remove any `DATABASE_URL` from `.env`. |
| 404 with an empty body | The path doesn't exist (e.g. a typo in `/healthz`). Only `/`, the frontend files, `/healthz` and `/api/*` exist. |
| Traefik answers 404 with no request in its access log for `cs2-pickem@…` | No router matches: check `DOMAIN` (B1) or the host in your dynamic config (B2), and that Traefik and `pickem-app` share `TRAEFIK_NETWORK`. |

| `docker compose up` fails with `bind source path does not exist: …/.docker/config.json` | Run `docker login ghcr.io` first (section 3), or point `DOCKER_CONFIG_FILE` in `.env` to your Docker `config.json`. |
| Watchtower logs `unauthorized` or `denied` for `ghcr.io` | The token in `config.json` expired or lacks `read:packages`. Log in again; Watchtower reads the file on every check. |

The SQLite database is `data/pickem.db` (with `-wal` and `-shm` while running). Backups are in `data/backups`. The image cache is `data/img-cache` and can be deleted at any time.

### Backups and restore

The app backs up its own database; there is no extra container. Each backup is a consistent snapshot taken while the app keeps running. It is checked with SQLite's `integrity_check`, compressed with zstd and stored in `data/backups`:

| File | When | Kept |
| --- | --- | --- |
| `pickem-<UTC time>-daily.db.zst` | Daily at `BACKUP_TIME` (default 04:00, in the time zone `TZ`, default UTC). If the Pi was off at that time, the backup runs right after the next start. | Together with the manual ones: the newest of each of the last 14 days, plus the newest of each of the last 8 weeks |
| `pickem-<UTC time>-manual.db.zst` | `/admin backup now` | As above |
| `pickem-<UTC time>-premigrate-v<NNNN>.db.zst` | At start-up, before a new release changes the database schema. `NNNN` is the schema version inside the file. | The newest 5 |
| `pickem-<UTC time>-prerestore.db.zst` | Before `app restore` replaces the database | The newest 5 |

- **A new release never migrates without a backup.** If the pre-migration backup fails (e.g. a full disk), the app logs the error and exits without touching the database. Docker keeps restarting it until the problem is fixed, or until you roll back with `IMAGE_TAG`.
- `/admin status` in Discord shows the last backup (time and size), the next daily run, and the last error. A failed backup is logged at level `ERROR` with `backup_failed=true`.
- **Second copy:** mount a NAS share (or any second disk) into the container and set `BACKUP_EXTRA_DIR`. Every backup is copied there and pruned by the same rules. In `docker-compose.yml`, uncomment `- /mnt/nas/pickem:/backup-extra`, then set `BACKUP_EXTRA_DIR=/backup-extra` in `.env`. The share must be writable by `APP_UID`/`APP_GID`. If the copy fails, the backup in `data/backups` still counts and `/admin status` shows the problem.
- A backup is a single file. Copy one off the Pi with e.g. `scp pi@<pi>:<deploy dir>/data/backups/pickem-…db.zst .`. Unpack it with `zstd -d` and open it with `sqlite3` to inspect it.
- Settings: `BACKUP_TIME` (`HH:MM` or `off`), `TZ`, `BACKUP_EXTRA_DIR`, `BACKUP_KEEP_DAILY`, `BACKUP_KEEP_WEEKLY`, `BACKUP_KEEP_SPECIAL`, `BACKUP_DIR` (default `/data/backups`). See `.env.example`.

**Restore** (replaces the current database; the current one is backed up first as `…-prerestore.db.zst`):

```sh
ls data/backups                                                 # pick a file
docker compose stop app
docker compose run --rm app restore pickem-20261122T030000Z-daily.db.zst
docker compose start app
```

A bare file name is looked up in `data/backups`; a path inside the container works too, e.g. `/backup-extra/pickem-…db.zst`. The file is checked before anything changes, so a damaged backup is refused and leaves the database untouched. If the current database is too damaged to back up, it is moved aside to `data/pickem.db.broken-<time>` instead. Restoring an older backup is fine across releases: on start, the app migrates it (with a pre-migration backup).

### Automatic updates (Watchtower)

Every green push to `main` builds a new image in GitHub Actions and pushes it to GHCR (`.github/workflows/ci.yml`). The `watchtower` container checks GHCR every 5 minutes. When the image behind `IMAGE_TAG` has changed, it pulls it, stops `app` gracefully, starts it again with the same settings, and removes the old image. `/healthz` then shows the new `git_sha`. Expect a merge to be live within about 20 minutes (CI run plus one check).

- **What it touches:** only containers labeled both `com.centurylinklabs.watchtower.enable=true` and `com.centurylinklabs.watchtower.scope=cs2-pickem`, which is `app` alone. Caddy, your Traefik, other stacks (even ones with their own Watchtower labels) and Watchtower itself are never updated. Bump Watchtower's version in `docker-compose.yml` by hand.
- **Channels** (`IMAGE_TAG` in `.env`):

  | `IMAGE_TAG` | What ships |
  | --- | --- |
  | `latest` (default) | every merge to `main` |
  | `stable` | only releases: the tag moves when a `vX.Y.Z` git tag is pushed (not for pre-releases like `v1.2.0-rc.1`) |
  | `vX.Y.Z` or `sha-<short>` | nothing; a fixed build, as for a rollback |

  To release: set `version` in `backend/Cargo.toml` (`[workspace.package]`) to `X.Y.Z` on `main`, then `git tag vX.Y.Z && git push origin vX.Y.Z`. CI refuses a tag that doesn't match.
- **Notifications (optional):** set `WATCHTOWER_NOTIFICATION_URL` in `.env` to a shoutrrr URL ([Watchtower notifications](https://watchtower.nickfedor.com/v1.24.1/configuration/notifications/)), then `docker compose up -d`. For a Discord webhook `https://discord.com/api/webhooks/<id>/<token>`, the URL is `discord://<token>@<id>`.
- **Database:** migrations stay backward-compatible for one release ([ADR 0003](../docs/adr/0003-database-schema.md)), so an automatic update or a rollback to the previous build never breaks the database.

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
docker compose up -d --wait app caddy   # Watchtower can't update a local image
curl -k https://localhost/healthz
```

Inside Discord this needs a real domain. To build an arm64 image on an amd64 machine, add `--platform linux/arm64`. The Dockerfile cross-compiles, so no QEMU is needed.
