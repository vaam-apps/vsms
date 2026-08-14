# syntax=docker/dockerfile:1
#
# Build context is the repository root, not backends/apps/sms-fake-orange/ — same
# reason as backends/apps/sms-gateway/Dockerfile and backends/apps/sms-worker/Dockerfile:
# `backends/crates/sms-fake-orange` sits under the same Cargo workspace and its own
# dependency graph is resolved against the root `Cargo.lock`. Build with
# `docker build -f backends/apps/sms-fake-orange/Dockerfile .` from the repo root.
#
# This image did not exist before the GHCR-only showcase compose stack
# (`compose.demo.yaml`) needed it: nothing in `.github/workflows/release.yml`
# built or published it, because every other consumer (`scripts/demo.sh`,
# `docs/runbooks/getting-started.md` step 6) runs it as a host binary via
# `cargo run`. A GHCR-only compose file has no `build:` stanza to fall back
# on, so this binary needs a real published image the same as the other
# four — see this Dockerfile's sibling comment in
# `.github/workflows/release.yml`'s own `build-and-push` matrix for the
# publishing side of this.
#
# **This image is a development/demo tool, not a production artifact.**
# `backends/apps/sms-fake-orange/src/main.rs`'s own module doc says so, loudly, and at
# WARN on every start: it impersonates Orange Cameroon's real HTTP API and
# sends no real SMS to any real handset. Never point a production deployment
# at it — this Dockerfile exists so a showcase/demo compose stack can pull a
# ready-made fake instead of building one from source, not to promote it to
# a supported production component.

# --- builder -----------------------------------------------------------
# Same base and reasoning as backends/apps/sms-gateway/Dockerfile and
# backends/apps/sms-worker/Dockerfile: Alpine's own libc is musl, so a plain
# `cargo build` here produces a static musl binary natively, no
# cross-compilation, no cross-linker. `sms-fake-orange` pulls in
# `wiremock` (a regular, not dev, dependency — see that crate's own
# Cargo.toml for why) but nothing in that dependency tree needs anything
# beyond what `ring` already requires here, confirmed by this build
# succeeding with the identical `musl-dev build-base` package set the
# other two Dockerfiles use — no `cmake`/`pkg-config`, `aws-lc-sys` is not
# in this graph either.
FROM rust:1.95-alpine3.22 AS builder

RUN apk add --no-cache musl-dev build-base

WORKDIR /app
COPY . .

# Same cache-mount ids as backends/apps/sms-gateway/Dockerfile and
# backends/apps/sms-worker/Dockerfile, deliberately — all three share the bulk of one
# dependency graph, so building any one of them warms the cache for the
# other two rather than starting cold every time.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-musl \
    --mount=type=cache,target=/app/target,id=cargo-target-workspace-musl \
    cargo build --release --locked -p sms-fake-orange-bin \
    && cp target/release/sms-fake-orange /tmp/sms-fake-orange

# --- runtime -------------------------------------------------------------
# distroless `static`/`nonroot` — same reasoning as the other two runtime
# binaries, including the `debian13` bump (see app/sms-gateway/Dockerfile's
# own comment for that evidence). No shell, no `curl`; this image also has
# no HEALTHCHECK for the same practical reason it never grew a
# `healthcheck` subcommand of its own (unlike sms-gateway/sms-worker):
# nothing in this stack needs to gate on its readiness before starting —
# dispatch only calls out to it lazily, at the first submit, not at any
# other service's own startup — so a compose file depending on this
# service only ever needs `service_started`, not `service_healthy`. See
# compose.demo.yaml's own comment on the `sms-fake-orange` service for that
# reasoning at the call site.
FROM gcr.io/distroless/static-debian13:nonroot AS runtime

COPY --from=builder /tmp/sms-fake-orange /usr/local/bin/sms-fake-orange

# main.rs's own CLI default (127.0.0.1:8090) is correct for a bare-metal
# run sharing a host with sms-gateway/sms-worker; inside a container that's
# this container's own network namespace, unreachable from a sibling
# container. `--bind-addr`/`FAKE_ORANGE_BIND_ADDR` is read at runtime by
# every caller of this image (compose.demo.yaml passes it explicitly), so
# no ENV default is baked in here the way SMS_LISTEN_ADDR is in the other
# two images — this binary has no other required-but-defaultable inputs to
# paper over, and `--dlr-endpoint`/`--sender-number` have no sane default
# at all (they must match whatever's calling this specific fake).
EXPOSE 8090

ENTRYPOINT ["/usr/local/bin/sms-fake-orange"]
