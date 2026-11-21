# T17 – CI/CD pipeline and automatic deployment

- **Epic:** E – Deployment and CI/CD
- **Status:** in review
- **Index:** [all tickets](README.md)

**Goal:** every green merge to `main` is running on the Pi a few minutes later, without manual steps.

**Depends on:** T16

**Scope**

- **GitHub Actions, PR workflow:** all quality gates from the conventions plus a frontend build. Required to merge.
- **Main workflow:** run the gates again, then build and push to GHCR with tags `latest`, `sha-<short>`, and `vX.Y.Z` on git tags. Nothing is pushed if a gate fails.
- **Build arm64 fast:** don't compile Rust under QEMU emulation, which is very slow. Cross-compile with `cargo-zigbuild` to `aarch64-unknown-linux-gnu` and copy the binary into the image, or use a native arm64 runner if your plan offers one. Cache with `Swatinem/rust-cache` and the buildx GHA cache. Also build amd64 for local testing.
- **Watchtower:** use `nickfedor/watchtower`, the maintained fork, because the original project is archived. Settings: label-enable mode (only labeled containers are updated), poll every 5 min, `--cleanup`, optional notification through a Discord webhook (shoutrrr URL). If the GHCR package is private, mount a Docker `config.json` with a read-only token.
- **Release channel (optional):** Watchtower follows `${IMAGE_TAG}`. With `latest` every merge ships; set `IMAGE_TAG=stable` and move that tag only on releases if you want control.
- Migrations stay backward-compatible for one release (conventions), so an update or a manual rollback never breaks the DB.

**Acceptance criteria**

- [ ] A merge to `main` changes `git_sha` in `/healthz` on the Pi within about 20 min.
- [ ] A failing test produces no image.
- [ ] A warm-cache pipeline run takes under 15 min.
- [x] Only the `app` container is updated by Watchtower; `caddy` (when its profile is enabled) stays pinned, and an existing Traefik (T16, option B) is never touched.

**Notes**

- Decisions: [ADR 0009](../adr/0009-ci-cd-and-watchtower.md).
  - One workflow, `.github/workflows/ci.yml`. The gate jobs `backend` and `frontend` run on every PR. `image` runs on `main`, on `v*.*.*` tags and on manual runs, and has `needs:` on both gate jobs.
  - Image tags: `latest` (main), `sha-<7>` (= `git_sha` in `/healthz`), `vX.Y.Z` and `stable` (releases without a pre-release suffix). A release tag must match the workspace version.
- **arm64 without QEMU:** the existing cross-compiling Dockerfile (ADR 0008) builds both platforms on one amd64 runner, instead of `cargo-zigbuild` or a native runner.
  - The Rust dependencies are now a cargo-chef layer, because BuildKit cache mounts never reach the GHA cache.
  - Checked locally: cold multi-arch build 3:00 min; after a source change, `cook` comes from the cache for both platforms and the build takes 56 s.
- **Watchtower** `nickfedor/watchtower:1.24.1`, always on: label-enable plus scope `cs2-pickem`, 5 min polling, cleanup, optional shoutrrr URL. GHCR credentials come from the host's `config.json` (read-only, `create_host_path: false`).
- **Owner decisions:** the repo and the GHCR package stay private. No required checks, because GitHub Free can't protect branches on private repos; ADR 0009 names the checks to require if that changes.
- **Watchtower end to end, checked locally on 21 Nov 2026** (Docker 29, local registry, stack with Caddy):
  - a multi-arch push of a new `latest` updated `pickem-app` at the next check: SIGTERM with the 20 s timeout from `stop_grace_period`, the same user, mounts, network aliases and labels, `/healthz` with the new `git_sha` through Caddy, and the old image removed;
  - untouched: Caddy, a decoy container with `watchtower.enable=true` but no scope (a new image was pushed for it too), Watchtower itself and unrelated stacks;
  - an empty `WATCHTOWER_NOTIFICATION_URL` is fine.
- Follow-ups:
  - Dependabot for the action, cargo-chef and Watchtower versions;
  - an image build on PRs, if Dockerfile breakage on `main` ever hurts;
  - native arm64 runners, if builds get slow.
