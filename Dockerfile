# syntax=docker/dockerfile:1
#
# CS2 Pick'em: one image with the backend binary and the built Activity (T16).
#
#   docker buildx build --platform linux/arm64 --build-arg GIT_SHA=$(git rev-parse --short HEAD) .
#
# Both build stages run on the build machine ($BUILDPLATFORM). Rust
# cross-compiles to the target architecture with Debian's cross gcc (for the
# C code in aws-lc-rs and the bundled SQLite), and the final stage only copies
# files, so building an arm64 image on amd64 needs no QEMU emulation.
# See docs/adr/0008-container-image-and-proxy.md.
#
# The Rust dependencies are compiled in their own layer (cargo-chef), so a
# layer cache (CI: buildx `type=gha`, T17) skips them until Cargo.toml or
# Cargo.lock change. See docs/adr/0009-ci-cd-and-watchtower.md.

ARG NODE_IMAGE=node:22-slim
ARG RUST_IMAGE=rust:1-slim-trixie
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian13:nonroot
ARG CARGO_CHEF_VERSION=0.1.78

# ---- frontend: pnpm build → /src/frontend/dist ---------------------------------
FROM --platform=$BUILDPLATFORM ${NODE_IMAGE} AS frontend
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable
WORKDIR /src/frontend
COPY frontend/package.json frontend/pnpm-lock.yaml ./
RUN --mount=type=cache,id=pnpm-store,target=/pnpm-store \
    pnpm config set store-dir /pnpm-store && \
    pnpm install --frozen-lockfile
COPY frontend/ ./
RUN pnpm build

# ---- chef: cargo-chef on the build machine, shared by all targets -------------
FROM --platform=$BUILDPLATFORM ${RUST_IMAGE} AS chef
ARG CARGO_CHEF_VERSION
ENV CARGO_TERM_COLOR=never
RUN cargo install cargo-chef --locked --version "${CARGO_CHEF_VERSION}" && \
    rm -rf /usr/local/cargo/registry
WORKDIR /src/backend

# ---- planner: the dependency recipe (changes only with manifests/lockfile) ----
FROM chef AS planner
COPY backend/ ./
RUN cargo chef prepare --recipe-path recipe.json

# ---- backend: cargo build --release for $TARGETARCH → /out/app ---------------
FROM chef AS backend
ARG BUILDARCH
ARG TARGETARCH
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-gnu;  gcc=gcc-x86-64-linux-gnu;  libc=libc6-dev-amd64-cross ;; \
      arm64) target=aarch64-unknown-linux-gnu; gcc=gcc-aarch64-linux-gnu; libc=libc6-dev-arm64-cross ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
      apt-get update; \
      apt-get install -y --no-install-recommends "$gcc" "$libc"; \
      rm -rf /var/lib/apt/lists/*; \
    fi; \
    rustup target add "$target"; \
    echo "$target" > /rust-target
# The triple-prefixed gcc exists natively and as a cross compiler, so these
# work for every build/target combination.
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar
# Dependencies only: this layer stays cached while the recipe is unchanged.
# No cache mounts here: BuildKit never exports them, so CI couldn't reuse them.
COPY --from=planner /src/backend/recipe.json recipe.json
RUN cargo chef cook --release --locked --target "$(cat /rust-target)" -p app --recipe-path recipe.json
COPY backend/ ./
# `.git` is not in the build context; build.rs reads GIT_SHA for /healthz.
ARG GIT_SHA=
ENV GIT_SHA=${GIT_SHA}
# Production build: no `dev` feature (no fixture source, no /api/dev/*).
RUN set -eux; \
    target="$(cat /rust-target)"; \
    cargo build --release --locked --target "$target" -p app; \
    mkdir -p /out/data; \
    cp "target/$target/release/app" /out/app

# ---- runtime ---------------------------------------------------------------------
# distroless/cc: glibc, CA certificates and the `nonroot` user (65532), no
# shell. Compose may run it as another UID (APP_UID) to match ./data.
FROM ${RUNTIME_IMAGE}
LABEL org.opencontainers.image.title="cs2-pickem" \
      org.opencontainers.image.description="Self-hosted Counter-Strike 2 pick'em for one Discord server" \
      org.opencontainers.image.licenses="MIT"
COPY --from=backend /out/app /app/app
COPY --from=frontend /src/frontend/dist /app/dist
# An empty, writable /data for runs without a mounted volume.
COPY --from=backend --chown=65532:65532 /out/data /data
ENV STATIC_DIR=/app/dist \
    DATABASE_URL=sqlite:///data/pickem.db \
    IMAGE_CACHE_DIR=/data/img-cache \
    BIND_ADDR=0.0.0.0:8080 \
    LOG_FORMAT=json \
    RUST_LOG=info
WORKDIR /app
USER 65532:65532
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --start-interval=2s --retries=3 \
    CMD ["/app/app", "healthcheck"]
ENTRYPOINT ["/app/app"]
