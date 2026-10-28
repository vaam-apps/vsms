# syntax=docker/dockerfile:1
#
# Build context is the repository root, not backends/apps/sms-gateway/ — build with
# `docker build -f backends/apps/sms-gateway/Dockerfile .` from the repo root.
# `include_server_schema!` resolves `schemas/vsms.cstack` via a path
# relative to backends/crates/sms-api (`../../schemas/vsms.cstack`), so the schema
# tree has to be in the build context.

# --- builder -----------------------------------------------------------
# Alpine, not Debian: Alpine's own libc *is* musl, so `cargo build` on this
# image produces a musl binary natively — no cross-compilation, no
# `rustup target add`, no cross-linker. The TLS stack is `ring` throughout
# (no `aws-lc-rs` anywhere in the graph), and `main.rs`'s
# `install_default_crypto_provider` names it before anything constructs an
# HTTP client. Same pin as the workspace's declared MSRV
# floor (`rust-version = "1.98.0"`, root `Cargo.toml`) and the exact
# release-line CI's `dtolnay/rust-toolchain@stable` currently resolves to;
# `3.22` pinned rather than the floating `-alpine` tag so this build
# doesn't silently pick up a new Alpine base (and therefore a new musl/gcc)
# on an unrelated day.
FROM rust:1.98-alpine3.22 AS builder

# musl-dev + build-base (gcc, make, ...): `ring` (the sole crypto provider
# in this graph) needs a C compiler and libc headers to build its
# assembly/C backend. Nothing here needs cmake or pkg-config: those existed
# solely for `aws-lc-sys`, which does not appear in the dependency graph
# (`cargo tree -i aws-lc-rs` reports no match).
# DL3018: no apk version pin, deliberately. Alpine drops a superseded package
# revision from the 3.22 index when it ships the next one, so an exact pin
# makes this build fail on the day the revision rotates out, for no change
# of ours. What this image does pin is the base: `rust:1.98-alpine3.22` fixes
# the Alpine release, and with it the musl/gcc series `build-base` resolves to.
# hadolint ignore=DL3018
RUN apk add --no-cache musl-dev build-base

WORKDIR /app
COPY . .

# --locked: fail rather than silently re-resolve if Cargo.lock and
# Cargo.toml have drifted, matching the guarantee ci.yml's own
# "Cargo.lock is up to date" step checks — a Docker build should not be
# the first place a lock drift is discovered.
#
# Cache mounts, not a plain layer: the registry cache is shared with
# backends/apps/sms-worker/Dockerfile under the same id (identical dependency graph,
# so a build of one warms the other), and the target-dir cache means a
# rebuild after a source-only change recompiles what changed, not the
# whole dependency tree. Neither is visible in the final image — the cache
# mount's contents never become a layer — so the binary is copied out to a
# real path before the RUN ends. `-musl` in both cache ids, distinct from
# any glibc-targeted cache that might exist on the same builder host, since
# the two produce incompatible `target/` artifacts.
# Cargo features to build with. Empty for every published image: the release
# workflow (.github/workflows/images.yml) never sets it. compose.dev.yaml sets
# `fake-aws` so `just demo` can accept receipts from sms-fake-aws; see that
# feature's comment in this crate's Cargo.toml.
ARG CARGO_FEATURES=""
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-musl \
    --mount=type=cache,target=/app/target,id=cargo-target-workspace-musl \
    cargo build --release --locked -p sms-gateway --features "$CARGO_FEATURES" \
    && cp target/release/sms-gateway /tmp/sms-gateway

# --- runtime -------------------------------------------------------------
# distroless `static`, not `cc` or `base`: the builder stage above produces
# a fully static musl binary (musl targets default to `crt-static`), so
# there is no libc for the runtime image to provide at all — `static` is
# the distroless variant built for exactly that (Go binaries, mainly, but
# nothing about it is Go-specific). `:nonroot` runs as uid/gid 65532
# without a `useradd`/`USER` line — there is no shell or `useradd` binary
# in this image to run one with. Confirmed by inspecting the image's own
# layers (`docker save`/`docker export` + `tar -tzv`, not assumed from
# documentation alone): it ships `/etc/passwd`, `/etc/group`,
# `/etc/nsswitch.conf`, a current `/etc/ssl/certs/ca-certificates.crt` (so
# outbound TLS to Orange's API needs no separate `ca-certificates` package
# the way the old `debian:bookworm-slim` runtime did), and `/tmp` at mode
# `1777` — this binary doesn't write to `/tmp`, but the sibling
# `backends/apps/sms-worker/Dockerfile` does, and the same base image is
# confirmed to support it.
#
# `debian13`, not `debian12` (bumped in the image-hygiene PR that added
# `backends/apps/vsms-demo-seed`): the distroless project (github.com/
# GoogleContainerTools/distroless) now publishes `static-debian13` as its
# current default — confirmed by pulling the real `gcr.io/distroless/
# static-debian13:nonroot` manifest and image, not assumed from memory or
# from the project's docs alone — and a `docker export` + `tar -tvf` of it
# reproduces every property this comment already lists for debian12
# byte-for-byte: uid/gid 65532 `nonroot` in `/etc/passwd`/`/etc/group`, the
# same three `/etc/passwd`/`/etc/group`/`/etc/nsswitch.conf` files, a
# current CA bundle, and `/tmp` (and `/var/tmp`) at mode `1777`
# (`drwxrwxrwt`). Debian 13 (trixie) is stable and the distroless project's
# own README now describes it as the current default rather than a
# preview, so this isn't an early adoption of an unreleased base.
FROM gcr.io/distroless/static-debian13:nonroot AS runtime

COPY --from=builder /tmp/sms-gateway /usr/local/bin/sms-gateway

# VSC-1: the YAML deployment configuration (config/application.yml and its
# profile overlays) is baked into the image, and SMS_CONFIG names the base file.
# The profile (SMS_PROFILE) is chosen by whoever runs the image, never baked:
# the default is `dev`, which points at local fakes and is never livemode.
# Nothing in these files is a secret - secrets are `${VAR}` placeholders
# (`cargo xtask config-literal-secrets`), delivered through the environment - so
# a layer holding them holds no credential. A deployment replaces one overlay by
# mounting a file over /config/application-<profile>.yml.
COPY --from=builder /app/config /config
ENV SMS_CONFIG=/config/application.yml

# main.rs's own default (127.0.0.1:8080) is correct for a bare-metal
# deploy where this process shares a host with its Caddy edge — inside a
# container, "loopback" is this container's own network namespace, so a
# sibling container (Caddy, in deploy/docker-compose.yml) could never reach
# it. Binding all interfaces here is still safe: nothing routes to this
# container's port from outside the compose network, and TLS still
# terminates at Caddy, never here.
ENV SMS_LISTEN_ADDR=0.0.0.0:8080
# #70/#71: same reasoning as SMS_LISTEN_ADDR immediately above — this
# process's own `--metrics-listen` default (127.0.0.1:9090) is correct
# bare-metal but unreachable from a sibling `prometheus` container
# (deploy/docker-compose.yml) without this override. Still never faces the
# internet: `deploy/Caddyfile`'s blanket `reverse_proxy sms-gateway:8080`
# names a different port entirely, and this one is never published to the
# host either (see deploy/docker-compose.yml's own comment on the
# `sms-gateway` service).
ENV SMS_METRICS_LISTEN_ADDR=0.0.0.0:9090
EXPOSE 8080 9090

# Exec-form (a JSON array, not a shell string): this image has no
# `/bin/sh` for a `CMD-SHELL`-style check to run, and no `curl` either.
# `sms-gateway healthcheck` (backends/apps/sms-gateway/src/main.rs) reproduces the
# old `curl -fsS http://127.0.0.1:8080/healthz` check as a plain HTTP/1.1
# GET over a raw socket — no shell, no third-party binary, nothing beyond
# `std` needed.
# Numeric uid:gid, not `nonroot:nonroot`. The `:nonroot` tag already runs as
# 65532 and ships /etc/passwd, so the user exists either way — but a *named*
# USER is something a kubelet cannot verify: a pod with `runAsNonRoot: true`
# and no `runAsUser` is refused with "image has non-numeric user (nonroot),
# cannot verify user is non-root". 65532 is the same uid, spelled so it can
# be checked. (Also what Trivy's DS-0002 wants: a literal, non-root USER.)
# `ci/assert-image.sh` fails the image build if this ever becomes a name again.
USER 65532:65532
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/sms-gateway", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/sms-gateway"]
CMD ["serve"]
