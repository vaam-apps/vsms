# syntax=docker/dockerfile:1
#
# Build context is the repository root — see app/sms-gateway/Dockerfile's
# own comment for why (schema/ has to be in context for
# include_server_schema!, which sms-worker's own dependency tree pulls in
# through sms-api). Build with
# `docker build -f app/sms-worker/Dockerfile .` from the repo root.

# --- builder -----------------------------------------------------------
# Same base, same reasoning as app/sms-gateway/Dockerfile: glibc, not musl
# (rustls-no-provider is unverified — see AGENTS.md). Also same toolchain
# version, and for the same reason found while building that image first:
# the workspace's declared `rust-version = "1.85"` does not actually build
# the committed Cargo.lock (`jsonwebtoken` → `time` needs rustc 1.88) — see
# that Dockerfile's own comment. Pinned to match what CI's floating
# `dtolnay/rust-toolchain@stable` currently resolves to instead.
FROM rust:1.95-bookworm AS builder

# cmake + pkg-config: aws-lc-sys, transitively via authkestra — see
# app/sms-gateway/Dockerfile's own comment; identical dependency graph here.
RUN apt-get update && apt-get install -y --no-install-recommends \
      cmake \
      pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

# The package is `sms-worker-bin`, not `sms-worker` — that name is taken by
# the library crate (crates/sms-worker). The `[[bin]]` override in
# app/sms-worker/Cargo.toml is what still produces `target/release/sms-worker`
# as the executable name, matching every `sms-worker --roles ...` example in
# the design doc.
#
# Same cache-mount reasoning as app/sms-gateway/Dockerfile — `cargo-registry`
# is the identical id, deliberately, so this build reuses crates already
# fetched/compiled for the gateway image rather than starting cold.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry \
    --mount=type=cache,target=/app/target,id=cargo-target-workspace \
    cargo build --release --locked -p sms-worker-bin \
    && cp target/release/sms-worker /tmp/sms-worker

# --- runtime -------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# ca-certificates: outbound TLS to Orange's API when --roles includes
# dispatch. No curl here — see the HEALTHCHECK comment below for why.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin sms-worker

COPY --from=builder /tmp/sms-worker /usr/local/bin/sms-worker

USER sms-worker
WORKDIR /

# This binary has no HTTP surface — six poll-loop roles, never a listener
# (see main.rs's own module doc) — so there is nothing for a curl-style
# HEALTHCHECK to hit, and bolting an HTTP server on just to answer a probe
# would be adding a listening port this deployment's threat model doesn't
# want. Instead, main.rs spawns a heartbeat task (independent of which
# --roles are selected) that touches this file every 15s; the check below
# treats a stale mtime as unhealthy. That is a real liveness signal a bare
# `pgrep sms-worker` would not be: a hung tokio runtime (a role genuinely
# deadlocked, not just idling on `run`'s own `std::future::pending` stub)
# stops updating the file while the process itself stays alive.
ENV SMS_WORKER_HEALTH_FILE=/tmp/sms-worker-healthy
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD sh -c 'f="$SMS_WORKER_HEALTH_FILE"; test -f "$f" && [ "$(( $(date +%s) - $(stat -c %Y "$f") ))" -lt 90 ]'

ENTRYPOINT ["/usr/local/bin/sms-worker"]
