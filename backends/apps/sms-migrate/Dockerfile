# syntax=docker/dockerfile:1
#
# Build context is the repository root — build with
# `docker build -f backends/apps/sms-migrate/Dockerfile .` from the repo root.
# `backends/apps/sms-migrate/build.rs` walks `backends/migrations/postgres/*/up.sql`
# at compile time and generates the `include_str!` calls `src/main.rs`
# embeds — so the whole `backends/migrations/postgres/` tree has to be in
# the build context, but there is nothing to `COPY` into the *runtime*
# image the way the old `deploy/migrate.Dockerfile` copied `.sql` files
# onto a `postgres:16-alpine` base — the SQL is already inside the binary.

# --- builder -----------------------------------------------------------
# Same base, same reasoning as backends/apps/sms-gateway/Dockerfile: Alpine's own
# libc is musl, so this produces a static musl binary natively.
FROM rust:1.95-alpine3.22 AS builder

# musl-dev + build-base: this binary's own dependency tree still resolves
# `ring` (via cratestack-sqlx's TLS feature — see AGENTS.md's "aws-lc-rs
# enters this tree" section), which needs a C compiler and libc headers to
# build.
RUN apk add --no-cache musl-dev build-base

WORKDIR /app
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-musl \
    --mount=type=cache,target=/app/target,id=cargo-target-workspace-musl \
    cargo build --release --locked -p sms-migrate \
    && cp target/release/sms-migrate /tmp/sms-migrate

# --- runtime -------------------------------------------------------------
# distroless `static`/`nonroot` — see backends/apps/sms-gateway/Dockerfile's own
# comment for what this image ships and why `:nonroot` needs no
# `useradd`/`USER` line, and for the `debian13` bump's own evidence. This
# binary makes no outbound HTTPS calls (only a Postgres connection, TLS or
# otherwise, via `cratestack-sqlx`'s bundled webpki roots — see that
# crate's own feature selection in the root `Cargo.toml`'s comments), so
# unlike sms-gateway/sms-worker it never actually needs the image's own
# `/etc/ssl/certs/ca-certificates.crt` — harmless that it's there
# regardless, not something this Dockerfile depends on.
FROM gcr.io/distroless/static-debian13:nonroot AS runtime

COPY --from=builder /tmp/sms-migrate /usr/local/bin/sms-migrate

# One-shot: applies the committed migrations (guarded by a Postgres
# advisory lock, safe to run concurrently or repeatedly — see
# backends/apps/sms-migrate/src/main.rs's own module doc) and exits. No listener, no
# HEALTHCHECK — deploy/docker-compose.yml's `migrate` service has
# `restart: 'no'` and every dependent service waits on
# `service_completed_successfully`, and the Helm chart's own `migrate` Job
# (deploy/charts/vsms/values.yaml) is a pre-install/pre-upgrade hook,
# neither of which probes a running container the way a long-lived
# Deployment does.
ENTRYPOINT ["/usr/local/bin/sms-migrate"]
