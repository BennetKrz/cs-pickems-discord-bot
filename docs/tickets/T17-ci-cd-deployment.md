# T17 – CI/CD pipeline and automatic deployment

- **Epic:** E – Deployment and CI/CD
- **Status:** todo
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
- [ ] Only the `app` container is updated by Watchtower; `caddy` (when its profile is enabled) stays pinned, and an existing Traefik (T16, option B) is never touched.
