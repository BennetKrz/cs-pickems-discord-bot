# 0009 – CI/CD pipeline, image tags and Watchtower

- **Status:** accepted
- **Date:** 2026-11-21
- **Ticket:** T17

## Context

T17 ships every green merge to `main` to the Pi without manual steps. The constraints:

- **Private repo and package.** The repo is private, and the owner keeps the GHCR package private too, so the Pi needs pull credentials.
  - On GitHub Free, private repos get 2-core hosted runners.
  - Free can't protect branches or require checks on private repos.
- **Cross-compiling Dockerfile.** ADR 0008 already cross-compiles Rust to arm64 with Debian's cross gcc, on `$BUILDPLATFORM`, without QEMU. The ticket's alternatives were `cargo-zigbuild` or a native arm64 runner.
- **The T16 Dockerfile cached Rust in BuildKit cache mounts.** BuildKit never exports cache mounts to a remote cache (`type=gha`). Every CI run would compile all dependencies for both architectures on 2 cores, which breaks the 15-minute target for a warm run.
- **Watchtower** (fork `nickfedor/watchtower` 1.24.1, docs checked 21 Nov 2026):
  - `WATCHTOWER_LABEL_ENABLE`, `WATCHTOWER_SCOPE` (label `com.centurylinklabs.watchtower.scope`), `WATCHTOWER_POLL_INTERVAL`, `WATCHTOWER_CLEANUP` and `WATCHTOWER_NOTIFICATION_URL` (shoutrrr);
  - the stop timeout defaults to 30 s, and a container's own stop timeout wins;
  - registry credentials come from a mounted `config.json`, with `DOCKER_CONFIG` pointing at its directory.

## Decision

1. **One workflow** `.github/workflows/ci.yml`.
   - The gate jobs `backend` and `frontend` run on every PR, on `main`, on `v*.*.*` tags and on manual runs.
     - `backend` runs `cargo fmt --check`, clippy with `-D warnings` and `cargo test` (with `Swatinem/rust-cache`).
     - `frontend` runs lint, typecheck, test and build.
   - The `image` job runs everywhere except on PRs and has `needs: [backend, frontend]`, so a red gate means no image.
   - PRs don't build the image, to save private-repo minutes.
2. **Image tags** on `ghcr.io/<owner>/cs2-pickem`:
   - `latest`: the default branch only;
   - `sha-<7>`: every build, the same value as `git_sha` in `/healthz`;
   - `vX.Y.Z`: release tags;
   - `stable`: release tags without a pre-release suffix.

   `metadata-action` runs with `latest=false` so that release tags don't move `latest`. A release tag must equal `v` + the workspace version in `backend/Cargo.toml`, or the image job fails. A manual run on a branch pushes only `sha-<7>`, for trying a branch build on the Pi.
3. **Build both architectures on one amd64 runner** with the existing cross-compiling Dockerfile. There is no QEMU, no zigbuild and no native arm64 runner.
   - **Dependencies are a layer.** The Dockerfile compiles them in their own layer with **cargo-chef** (`cook` per target, after a shared `chef`/`planner` stage). With buildx `cache-to: type=gha,mode=max`, that layer stays cached until `Cargo.toml` or `Cargo.lock` change. The image's contents don't change.
   - **No cache mounts.** The `target` and registry cache mounts are removed.
4. **Watchtower in the compose stack** (always on, pinned version):
   - `WATCHTOWER_LABEL_ENABLE=true`, `WATCHTOWER_SCOPE=cs2-pickem`, a 300 s poll interval, `WATCHTOWER_CLEANUP=true`, no startup message, and an optional notification URL.
   - `app` carries both the enable label and the scope label.
   - Watchtower carries only the scope label, so it never updates itself.
   - The label-enable mode alone would still update containers from other stacks that carry `watchtower.enable=true`; the scope stops that. Caddy and Traefik have neither label.
   - The stop timeout comes from `app`'s `stop_grace_period: 20s`.
5. **Pull credentials:** a classic PAT with only `read:packages`, from `docker login ghcr.io` on the Pi. Watchtower mounts the resulting `config.json` read-only (`DOCKER_CONFIG=/`). The mount uses `create_host_path: false`, so a missing file fails loudly instead of Docker creating a directory at that path.
6. **No merge enforcement** (GitHub Free, private repo). CI runs on every PR, and merging on red is a process rule, not a technical block.

## Consequences

- A warm pipeline run is two gate jobs in parallel, then two incremental release builds of the workspace crates. Dependencies come from the GHA cache.
- If `Cargo.lock` changes, both `cook` layers rebuild: one slow run.
- The GHA cache is limited to 10 GB per repo. The two `cook` layers plus rust-cache fit, and old entries are evicted.
- Local `docker buildx build` gets the same layer caching. It no longer reuses the incremental target directory from a cache mount, so a change to the dependencies recompiles all of them locally too.
- **Pinning pauses updates.** Watchtower follows `IMAGE_TAG`, so pinning `sha-<short>` or `vX.Y.Z` is both the rollback and the way to pause updates. `stable` gives a release channel without any workflow change.
- If the PAT expires, updates stop silently, apart from Watchtower's log and an optional notification. It has to be renewed by hand.
- Watchtower itself, the actions and cargo-chef are pinned. Bumping them is manual for now (a Dependabot follow-up).
- If the repo goes public or the plan gets protected branches, require the checks `backend (fmt, clippy, test)` and `frontend (lint, typecheck, test, build)` on `main`.
- **Migrations.** As in ADR 0003, migrations stay backward-compatible for one release, so an unattended update or a rollback to the previous build keeps working on the same database.
