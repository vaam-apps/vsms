# syntax=docker/dockerfile:1
#
# Build context is the repository root:
# `docker build -f backends/apps/sms-fake-aws/Dockerfile .`
#
# A development/demo fake of AWS End User Messaging SMS and its SNS receipts
# (backends/apps/sms-fake-aws/src/main.md). Built by compose.dev.yaml only;
# never published. Same two-stage shape as backends/apps/sms-fake-orange's,
# for the same reasons (see backends/apps/sms-gateway/Dockerfile).
FROM rust:1.98-alpine3.22 AS builder

# DL3018: see backends/apps/sms-gateway/Dockerfile.
# hadolint ignore=DL3018
RUN apk add --no-cache musl-dev build-base

WORKDIR /app
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-musl \
    --mount=type=cache,target=/app/target,id=cargo-target-workspace-musl \
    cargo build --release --locked -p sms-fake-aws-bin \
    && cp target/release/sms-fake-aws /tmp/sms-fake-aws

FROM gcr.io/distroless/static-debian13:nonroot AS runtime

COPY --from=builder /tmp/sms-fake-aws /usr/local/bin/sms-fake-aws

EXPOSE 8095

USER 65532:65532
ENTRYPOINT ["/usr/local/bin/sms-fake-aws"]
