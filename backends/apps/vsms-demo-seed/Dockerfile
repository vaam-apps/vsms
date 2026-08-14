# syntax=docker/dockerfile:1
#
# Build context is the repository root, not backends/apps/vsms-demo-seed/ — same
# reason as every other Dockerfile in this workspace: `schema/` has to be in
# the build context for `include_server_schema!` (pulled in transitively
# through sms-api), and the whole workspace is resolved against one root
# `Cargo.lock`. Build with `docker build -f backends/apps/vsms-demo-seed/Dockerfile .`
# from the repo root.
#
# **This image is a development/demo tool, not a production artifact** —
# see backends/apps/vsms-demo-seed/src/main.rs's own module doc for the full
# reasoning on why this exists as its own binary/image rather than a
# subcommand of `sms-gateway` (it used to be one: `Command::SeedDemoApp`).
# Never point this at a production database: it seeds an auto-approved
# `SenderIdRegistration` with no real provider approval behind it, purely
# to unblock `compose.demo.yaml`'s showcase.

# --- builder -----------------------------------------------------------
# Same base and reasoning as every other Dockerfile in this workspace:
# Alpine's own libc is musl, so a plain `cargo build` here produces a
# static musl binary natively, no cross-compilation, no cross-linker.
FROM rust:1.95-alpine3.22 AS builder

# musl-dev + build-base: `ring` (pulled in transitively through sms-api ->
# authkestra/cratestack-sqlx) needs a C compiler and libc headers to build
# — identical package set every other Dockerfile in this workspace uses.
RUN apk add --no-cache musl-dev build-base

WORKDIR /app
COPY . .

# Same cache-mount ids as every other Dockerfile in this workspace,
# deliberately — this binary shares the bulk of its dependency graph with
# sms-gateway (both depend on sms-api directly), so building either warms
# the cache for the other rather than starting cold every time.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-musl \
    --mount=type=cache,target=/app/target,id=cargo-target-workspace-musl \
    cargo build --release --locked -p vsms-demo-seed \
    && cp target/release/vsms-demo-seed /tmp/vsms-demo-seed

# --- runtime -------------------------------------------------------------
# distroless `static`/`nonroot` — same reasoning as every other runtime
# image in this workspace: no shell, no package manager, a real
# `/etc/ssl/certs/ca-certificates.crt` (unused here — this binary only ever
# talks to Postgres, never HTTPS — but harmless to ship), and a `1777`
# `/tmp` this binary doesn't need either. `debian13`, not `debian12` — see
# this PR's own description for the evidence (a real `docker pull` +
# `docker export` + layer inspection, not assumed): the distroless project
# now publishes `static-debian13` as its current default, and it carries
# the identical uid-65532 `nonroot` user, `/etc/passwd`/`/etc/group`/
# `/etc/nsswitch.conf`, CA bundle, and `1777 /tmp` verified for `debian12`.
# This is a new image with no debian12 precedent to stay pinned to, so it
# starts on the newer base directly.
FROM gcr.io/distroless/static-debian13:nonroot AS runtime

COPY --from=builder /tmp/vsms-demo-seed /usr/local/bin/vsms-demo-seed

# One-shot, no listener, no HEALTHCHECK — same shape as app/sms-migrate's
# own runtime stage: this binary seeds a handful of rows and exits, and
# compose.demo.yaml's own dependency chain gates on
# `service_completed_successfully`, not a running/healthy container.
ENTRYPOINT ["/usr/local/bin/vsms-demo-seed"]
