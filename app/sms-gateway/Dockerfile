# syntax=docker/dockerfile:1
#
# Build context is the repository root, not app/sms-gateway/ — build with
# `docker build -f app/sms-gateway/Dockerfile .` from the repo root.
# `include_server_schema!` resolves `schema/schema.cstack` via a path
# relative to crates/sms-api (`../../schema/schema.cstack`), so the schema
# tree has to be in the build context.

# --- builder -----------------------------------------------------------
# Debian glibc, matching the debian:bookworm-slim runtime below. A static
# musl/scratch build is deliberately not attempted here — AGENTS.md records
# that the rustls-no-provider path needed for it panics at
# `reqwest::Client::new()` unless the application installs a
# `CryptoProvider` in `main` first, and that runtime path is unverified.
# Shipping a glibc image that works beats shipping a musl one that panics
# on its first outbound call.
#
# NOT pinned to Cargo.toml's own declared `rust-version = "1.85"` — tried
# that first, and the build fails: `jsonwebtoken@11.0.0` (via `time`)
# actually requires rustc 1.88 through the committed Cargo.lock, a real gap
# between the declared MSRV floor and what the lockfile needs that
# `ci.yml`'s own `dtolnay/rust-toolchain@stable` (always latest, currently
# 1.95) never surfaces because it never builds at exactly 1.85. Pinned
# instead to the same stable line CI actually builds with, so this image
# matches what's actually proven to compile rather than a stale MSRV
# claim. Worth a follow-up to correct `rust-version` in the workspace
# Cargo.toml — not done here, out of scope for a deploy PR.
FROM rust:1.95-bookworm AS builder

# cmake + pkg-config: aws-lc-sys — pulled in transitively by authkestra's
# reqwest 0.13 default features (AGENTS.md: "aws-lc-rs enters this tree
# from authkestra alone") — builds AWS-LC from source and needs both.
# Confirmed against Cargo.lock: aws-lc-sys's own dependency list is
# cc, cmake, dunce, fs_extra, pkg-config — no libclang/bindgen involved, so
# no extra headers beyond a C toolchain (already in this base image).
RUN apt-get update && apt-get install -y --no-install-recommends \
      cmake \
      pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

# --locked: fail rather than silently re-resolve if Cargo.lock and
# Cargo.toml have drifted, matching the guarantee ci.yml's own
# "Cargo.lock is up to date" step checks — a Docker build should not be
# the first place a lock drift is discovered.
#
# Cache mounts, not a plain layer: the registry cache is shared with
# app/sms-worker/Dockerfile under the same id (identical dependency graph,
# so a build of one warms the other), and the target-dir cache means a
# rebuild after a source-only change recompiles what changed, not the
# whole dependency tree. Neither is visible in the final image — the cache
# mount's contents never become a layer — so the binary is copied out to a
# real path before the RUN ends.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry \
    --mount=type=cache,target=/app/target,id=cargo-target-workspace \
    cargo build --release --locked -p sms-gateway \
    && cp target/release/sms-gateway /tmp/sms-gateway

# --- runtime -------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# ca-certificates: every outbound TLS call this binary makes (Orange's API,
# and any future provider) needs a trust root; the builder stage's own Rust
# toolchain does not ship one into the release binary.
# curl: backs the HEALTHCHECK below.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin sms-gateway

COPY --from=builder /tmp/sms-gateway /usr/local/bin/sms-gateway

USER sms-gateway
WORKDIR /

# main.rs's own default (127.0.0.1:8080) is correct for a bare-metal
# deploy where this process shares a host with its Caddy edge — inside a
# container, "loopback" is this container's own network namespace, so a
# sibling container (Caddy, in deploy/docker-compose.yml) could never reach
# it. Binding all interfaces here is still safe: nothing routes to this
# container's port from outside the compose network, and TLS still
# terminates at Caddy, never here.
ENV SMS_LISTEN_ADDR=0.0.0.0:8080
EXPOSE 8080

# #139: this repo had no health endpoint for sms-gateway at all before
# app/sms-gateway/src/health.rs. Liveness only — see that module's own doc
# for why it doesn't touch the database.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["/usr/local/bin/sms-gateway"]
CMD ["serve"]
