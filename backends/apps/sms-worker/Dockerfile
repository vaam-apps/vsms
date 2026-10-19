# syntax=docker/dockerfile:1
#
# Build context is the repository root — see backends/apps/sms-gateway/Dockerfile's
# own comment for why (schema/ has to be in context for
# include_server_schema!, which sms-worker's own dependency tree pulls in
# through sms-api). Build with
# `docker build -f backends/apps/sms-worker/Dockerfile .` from the repo root.

# --- builder -----------------------------------------------------------
# Same base, same reasoning as backends/apps/sms-gateway/Dockerfile: Alpine's own
# libc is musl, so this produces a static musl binary natively, no
# cross-compilation. See that Dockerfile's own comment for why a musl
# build is safe now (rustls-no-provider + an explicit `CryptoProvider`
# install in `main`, proven against a real TLS handshake by
# backends/apps/sms-gateway/tests/tls_no_provider_live.rs — that test lives in the
# sibling package because it needs a second, differently-featured `reqwest`
# dev-dependency, but it proves the exact mechanism both binaries' `main.rs`
# rely on).
FROM rust:1.98-alpine3.22 AS builder

# musl-dev + build-base: `ring` needs a C compiler and libc headers — see
# backends/apps/sms-gateway/Dockerfile's own comment; identical dependency graph
# here. No cmake/pkg-config: `aws-lc-sys` is gone from the graph entirely.
# DL3018: no apk version pin, deliberately. Alpine drops a superseded package
# revision from the 3.22 index when it ships the next one, so an exact pin
# makes this build fail on the day the revision rotates out, for no change
# of ours. What this image does pin is the base: `rust:1.98-alpine3.22` fixes
# the Alpine release, and with it the musl/gcc series `build-base` resolves to.
# hadolint ignore=DL3018
RUN apk add --no-cache musl-dev build-base

WORKDIR /app
COPY . .

# The package is `sms-worker-bin`, not `sms-worker` — that name is taken by
# the library crate (backends/crates/sms-worker). The `[[bin]]` override in
# backends/apps/sms-worker/Cargo.toml is what still produces `target/release/sms-worker`
# as the executable name, matching every `sms-worker --roles ...` example in
# the design doc.
#
# Same cache-mount reasoning as backends/apps/sms-gateway/Dockerfile — `cargo-registry-musl`
# is the identical id, deliberately, so this build reuses crates already
# fetched/compiled for the gateway image rather than starting cold.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-musl \
    --mount=type=cache,target=/app/target,id=cargo-target-workspace-musl \
    cargo build --release --locked -p sms-worker-bin \
    && cp target/release/sms-worker /tmp/sms-worker

# --- runtime -------------------------------------------------------------
# distroless `static`/`nonroot` — see backends/apps/sms-gateway/Dockerfile's own
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

# #70/#71: same reasoning as backends/apps/sms-gateway/Dockerfile's own
# SMS_METRICS_LISTEN_ADDR override — main.rs's own `--metrics-listen`
# default (127.0.0.1:9091) is correct bare-metal but unreachable from a
# sibling `prometheus` container without this. Never published to the host
# (see deploy/docker-compose.yml's own `sms-worker` service).
ENV SMS_WORKER_METRICS_LISTEN_ADDR=0.0.0.0:9091
EXPOSE 9091

# Exec-form: this image has no `/bin/sh`, so the old
# `sh -c 'test -f ... && [ ... -lt 90 ]'` check can no longer run at all.
# `sms-worker healthcheck` (backends/apps/sms-worker/src/main.rs) reproduces that
# exact check — same file, same 90s staleness threshold
# (`HEALTH_STALE_THRESHOLD`) — in std-only Rust, intercepted ahead of the
# normal `--roles`-driven startup path rather than added as a real clap
# subcommand (see that interception's own comment for why).
# Numeric uid:gid, not `nonroot:nonroot`. The `:nonroot` tag already runs as
# 65532 and ships /etc/passwd, so the user exists either way — but a *named*
# USER is something a kubelet cannot verify: a pod with `runAsNonRoot: true`
# and no `runAsUser` is refused with "image has non-numeric user (nonroot),
# cannot verify user is non-root". 65532 is the same uid, spelled so it can
# be checked. (Also what Trivy's DS-0002 wants: a literal, non-root USER.)
# `ci/assert-image.sh` fails the image build if this ever becomes a name again.
USER 65532:65532
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD ["/usr/local/bin/sms-worker", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/sms-worker"]
