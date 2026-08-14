# syntax=docker/dockerfile:1
#
# Build context is the repository root — see app/sms-gateway/Dockerfile's
# own comment for why (schema/ has to be in context for
# include_server_schema!, which sms-worker's own dependency tree pulls in
# through sms-api). Build with
# `docker build -f app/sms-worker/Dockerfile .` from the repo root.

# --- builder -----------------------------------------------------------
# Same base, same reasoning as app/sms-gateway/Dockerfile: Alpine's own
# libc is musl, so this produces a static musl binary natively, no
# cross-compilation. See that Dockerfile's own comment for why a musl
# build is safe now (rustls-no-provider + an explicit `CryptoProvider`
# install in `main`, proven against a real TLS handshake by
# app/sms-gateway/tests/tls_no_provider_live.rs — that test lives in the
# sibling package because it needs a second, differently-featured `reqwest`
# dev-dependency, but it proves the exact mechanism both binaries' `main.rs`
# rely on).
FROM rust:1.95-alpine3.22 AS builder

# musl-dev + build-base: `ring` needs a C compiler and libc headers — see
# app/sms-gateway/Dockerfile's own comment; identical dependency graph
# here. No cmake/pkg-config: `aws-lc-sys` is gone from the graph entirely.
RUN apk add --no-cache musl-dev build-base

WORKDIR /app
COPY . .

# The package is `sms-worker-bin`, not `sms-worker` — that name is taken by
# the library crate (crates/sms-worker). The `[[bin]]` override in
# app/sms-worker/Cargo.toml is what still produces `target/release/sms-worker`
# as the executable name, matching every `sms-worker --roles ...` example in
# the design doc.
#
# Same cache-mount reasoning as app/sms-gateway/Dockerfile — `cargo-registry-musl`
# is the identical id, deliberately, so this build reuses crates already
# fetched/compiled for the gateway image rather than starting cold.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-musl \
    --mount=type=cache,target=/app/target,id=cargo-target-workspace-musl \
    cargo build --release --locked -p sms-worker-bin \
    && cp target/release/sms-worker /tmp/sms-worker

# --- runtime -------------------------------------------------------------
# distroless `static`/`nonroot` — see app/sms-gateway/Dockerfile's own
# comment for what this image does and doesn't ship (no shell, current CA
# certs, and a `1777` `/tmp` this binary's own heartbeat file below
# actually depends on — confirmed by inspecting the image's layers
# directly, not assumed) and for the `debian13` bump's own evidence (same
# uid, same `/tmp` mode, verified against the real published image before
# switching).
FROM gcr.io/distroless/static-debian13:nonroot AS runtime

COPY --from=builder /tmp/sms-worker /usr/local/bin/sms-worker

# This binary has a `/metrics` listener now (#70/#71, main.rs's own module
# doc) but still no HTTP route an orchestrator's own liveness probe could
# usefully hit — `/metrics` answers a different question (what the counters
# currently read), and bolting a liveness endpoint onto it would still be
# adding surface this deployment's threat model doesn't want just to answer
# a probe. Instead, main.rs spawns a heartbeat task (independent of which
# --roles are selected) that touches this file every 15s; the check below
# treats a stale mtime as unhealthy. That is a real liveness signal a bare
# `pgrep sms-worker` (or a still-answering `/metrics`, for that matter —
# two independent tokio tasks) would not be: a hung tokio runtime (a role
# genuinely deadlocked, not just idling on `run`'s own
# `std::future::pending` stub) stops updating the file while the process
# itself stays alive.
ENV SMS_WORKER_HEALTH_FILE=/tmp/sms-worker-healthy

# #70/#71: same reasoning as app/sms-gateway/Dockerfile's own
# SMS_METRICS_LISTEN_ADDR override — main.rs's own `--metrics-listen`
# default (127.0.0.1:9091) is correct bare-metal but unreachable from a
# sibling `prometheus` container without this. Never published to the host
# (see deploy/docker-compose.yml's own `sms-worker` service).
ENV SMS_WORKER_METRICS_LISTEN_ADDR=0.0.0.0:9091
EXPOSE 9091

# Exec-form: this image has no `/bin/sh`, so the old
# `sh -c 'test -f ... && [ ... -lt 90 ]'` check can no longer run at all.
# `sms-worker healthcheck` (app/sms-worker/src/main.rs) reproduces that
# exact check — same file, same 90s staleness threshold
# (`HEALTH_STALE_THRESHOLD`) — in std-only Rust, intercepted ahead of the
# normal `--roles`-driven startup path rather than added as a real clap
# subcommand (see that interception's own comment for why).
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD ["/usr/local/bin/sms-worker", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/sms-worker"]
