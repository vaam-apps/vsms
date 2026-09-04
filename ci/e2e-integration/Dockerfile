# syntax=docker/dockerfile:1
#
# `vsms-e2e-integration` — see `vsms-e2e-integration/src/main.rs`'s own
# module doc for why this has to run *inside* the Compose network rather
# than as a host process.
#
# Build context is the **repository root**, not this directory — unlike
# `deploy/backup-tool` (a fully standalone crate with no path
# dependencies), `ci/e2e-integration/vsms-e2e-integration` depends on
# `../../../sdks/rust/vsms-sdk-rust` by relative path, so that directory
# has to be inside the build context too. Build with:
#   docker build -f ci/e2e-integration/Dockerfile .

FROM rust:1.98-alpine3.22 AS builder

RUN apk add --no-cache musl-dev build-base

WORKDIR /app
COPY sdks/rust/vsms-sdk-rust/ sdks/rust/vsms-sdk-rust/
COPY ci/e2e-integration/ ci/e2e-integration/

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-e2e-integration \
    --mount=type=cache,target=/app/ci/e2e-integration/target,id=cargo-target-e2e-integration \
    cd ci/e2e-integration \
    && cargo build --release --locked -p vsms-e2e-integration \
    && cp target/release/vsms-e2e-integration /tmp/vsms-e2e-integration

# --- runtime -------------------------------------------------------------
# distroless `static`/`nonroot` — same reasoning as app/sms-gateway's own
# runtime stage: this binary only ever speaks plain HTTP to `sms-gateway`
# over the Compose network, needs no shell, and every credential it reads
# is a bind-mounted file path, not a command it execs.
FROM gcr.io/distroless/static-debian12:nonroot AS runtime

COPY --from=builder /tmp/vsms-e2e-integration /usr/local/bin/vsms-e2e-integration

ENTRYPOINT ["/usr/local/bin/vsms-e2e-integration"]
