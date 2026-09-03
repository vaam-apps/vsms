# The single build environment for `just ci` — everything a contributor
# needs to run the ENTIRE CI gate locally, with nothing installed on the
# host beyond `docker`, `docker compose` and `just`. Mirrors what
# `.github/workflows/ci.yml`'s jobs install piecemeal across five different
# runners, in one pinned image, so "it passed in the container" and "it
# passes in CI" are the same claim.
#
# glibc, not musl/Alpine — this is a *build and test* environment for the
# whole workspace (including the admin console's Next.js build and every
# crate's own `cargo test`), not one of the release Dockerfiles under
# `backends/apps/*/Dockerfile`, which build musl-static binaries onto
# `distroless` for a different reason (see AGENTS.md's "rustls, musl, and
# distroless" section). `rust:1.95-bookworm` matches the MSRV this repo
# pins (`rust-version = "1.95.0"`, root Cargo.toml) and the Dockerfiles'
# own `rust:1.95-bookworm` builder stage.
FROM rust:1.95-bookworm

# CARGO_BUILD_JOBS default matches AGENTS.md's own "Build cost" warning:
# `include_server_schema!` expansion is memory-hungry enough to OOM at
# cargo's default job count on a constrained machine. This is only ever
# the *image's* own default — the actual value every `just ci`/`ci-inner`
# cargo invocation runs with is set by `justfile`'s own `jobs`/`_cargo`
# variables (which read `CARGO_BUILD_JOBS` from the environment, so
# `CARGO_BUILD_JOBS=8 just ci` overrides it — see that file's own comment
# on `jobs :=`), not this `ENV` line — this is what a bare, non-`just`
# process in the container falls back to.
ENV CARGO_BUILD_JOBS=4 \
    CARGO_INCREMENTAL=0 \
    DEBIAN_FRONTEND=noninteractive

# rustfmt/clippy: the `rust` and `rules` CI jobs both need them.
RUN rustup component add rustfmt clippy

# postgresql-client from the PGDG apt repo, not Debian bookworm's own
# (which ships PostgreSQL 15's client, not 16) — sms-test-support and the
# `migrations`/`live` CI jobs alike shell out to a real `psql`, and this
# repo's own Postgres everywhere else is 16 (`compose.yml`, `compose.dev.yaml`,
# the `postgres:16` CI service container). A client that's merely
# wire-compatible is not the same claim as "matches what CI actually runs".
#
# Node 26 (nodesource) — matches root package.json's `engines.node` (`>=26`)
# and the `actions/setup-node@v4` `node-version: '26'` every relevant CI job
# uses. pnpm is pinned via corepack from `packageManager` in package.json,
# not installed as a separate version here, so a package.json bump can never
# leave this image on a stale pnpm the way a hardcoded `npm i -g pnpm@X`
# would.
#
# corepack is no longer bundled with Node from 25.x onward (it shipped
# through Node 24 but was dropped from core starting 25 — confirmed live,
# not assumed: this exact `nodejs` 26.8.1 package leaves no `corepack`
# binary on PATH, so `corepack enable` right after install fails `not
# found`, exit 127). `npm install -g corepack` puts it back before
# `corepack enable` runs — npm itself still ships with Node 26.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl gnupg lsb-release git unzip \
    && install -d /usr/share/postgresql-common/pgdg \
    && curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc \
        -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc \
    && echo "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] \
        https://apt.postgresql.org/pub/repos/apt $(lsb_release -cs)-pgdg main" \
        > /etc/apt/sources.list.d/pgdg.list \
    && apt-get update && apt-get install -y --no-install-recommends postgresql-client-16 \
    && curl -fsSL https://deb.nodesource.com/setup_26.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && npm install -g corepack \
    && corepack enable \
    && rm -rf /var/lib/apt/lists/*

# `just` — the one task runner both this repo's own recipes and `ci-inner`
# go through. Installed from the upstream install script (pinned to a real
# release tag, not `@latest`), not `cargo install just` — building it from
# source would add several minutes to every fresh image build for a tool
# that ships prebuilt binaries.
RUN curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
    | bash -s -- --to /usr/local/bin --tag 1.21.0

# cargo-deny, via cargo-binstall (a prebuilt binary install, not a
# from-source compile) — the `deny` CI job's own tool, checked against all
# five manifests inside `ci-inner`. Both moving pieces here are pinned, not
# floating: the installer script itself is fetched at a tagged ref
# (`v1.22.0`, not `main`, which could change what it fetches or how,
# out from under this image on a rebuild with no Dockerfile change at all
# — the exact unpinned-dependency-in-a-pipeline shape AGENTS.md's own
# release-engineering notes warn about elsewhere), `BINSTALL_VERSION`
# makes that same installer fetch the matching pinned `cargo-binstall`
# release rather than whatever `releases/latest` resolves to on the day
# this image happens to build, and `cargo binstall --version` pins
# `cargo-deny` itself (`0.20.2`, matching `cargo deny --version` at the
# time this file was written — bump both together, deliberately, not let
# one drift).
RUN curl -L --proto '=https' --tlsv1.2 -sSf \
        https://raw.githubusercontent.com/cargo-bins/cargo-binstall/v1.22.0/install-from-binstall-release.sh \
    | BINSTALL_VERSION=v1.22.0 bash \
    && cargo binstall --no-confirm --version 0.20.2 cargo-deny

# The cratestack CLI, pinned to CRATESTACK_VERSION above. A Dockerfile
# build ARG cannot itself run `cargo xtask cratestack-pin` (there is no
# checked-out repo yet at this layer, and xtask depends on nothing that
# would justify a chicken-and-egg COPY-then-build here) — so this value is
# a hardcoded default that MUST be kept in step with root Cargo.toml's own
# cratestack pin by hand. `ci-inner` below re-derives the pin from the
# checked-out repo at run time and fails loudly, before running anything
# else, if the two disagree — see that recipe's first step.
ARG CRATESTACK_VERSION=0.8.10
RUN cargo install cratestack-cli --version "=${CRATESTACK_VERSION}" --locked

# `runner` in compose.test.yaml runs as a non-root uid:gid — a container
# writing into the bind-mounted repo as root would leave root-owned files
# in the actual host working tree (see that file's own comment). That uid
# is NOT always 1000: found in review, not assumed — GitHub-hosted runners
# (and plenty of real developer machines) use 1001 or something else
# entirely, and compose.test.yaml's own `user:` already reads
# `VSMS_CI_UID`/`VSMS_CI_GID` for exactly this reason. `CI_UID`/`CI_GID`
# build ARGs (default 1000, matching the compose file's own default) let
# `useradd`/`chown` below create the SAME uid:gid the container will
# actually run as — a mismatch here reproduces the identical
# Permission-denied failure this whole comment block exists to prevent,
# just for a different uid than 1000. Overriding the host uid means
# rebuilding this image with the matching build args (`just ci-build` —
# see that recipe's own comment, and docs/runbooks/testing.adoc), not just
# re-running `compose.test.yaml` with a different `VSMS_CI_UID` — the
# users this `RUN` creates are baked into the image layer, not
# re-evaluated at container start.
#
# `/home/ci` doesn't exist yet, and compose.test.yaml mounts named volumes
# over it (`ci_home`, `pnpm_store`) that need pre-existing, correctly-owned
# content at each exact path for Docker to seed the volume from on first
# mount — a named volume mounted at a path with no real content in the
# image gets its mount-point directory auto-created by the Docker daemon
# itself, as root, regardless of the container's own configured `user:`.
# Reproduced live, not assumed, for `/usr/local/cargo/registry` below —
# and a second time, differently, for `/repo/target`: that one is NOT a
# named volume (see compose.test.yaml's own comment on `CARGO_TARGET_DIR`
# for why — a script this repo already ships,
# `ci/assert-client-routes-match-server.mjs`, hardcodes
# `target/debug/sms-gateway` relative to the repo root), so it's a plain
# subdirectory of the bind-mounted `/repo` instead, inheriting whatever
# uid already owns the host tree — nothing to `chown` here for it at all.
#
# `CARGO_HOME`/`RUSTUP_HOME` (`/usr/local/cargo`, `/usr/local/rustup`) are
# NOT already world-writable in a way that matters here — found live, not
# assumed: the `cargo_registry`/`cargo_git` named volumes in
# compose.test.yaml mount over `/usr/local/cargo/registry`/`.../git`, and
# Docker seeds a fresh named volume from the image's own directory content
# at that path, ownership and permissions included. The `cargo install
# cratestack-cli` step above ran as root and left that content root-owned;
# the base image's own broad `chmod` (if any) does not survive being
# re-populated by a later `RUN` under this uid:gid pair. First real run
# reproduced exactly this: `failed to open
# .../registry/cache/.../memchr-2.8.3.crate: Permission denied (os error
# 13)` the moment `cargo test` (running as uid 1000) tried to read a
# root-owned cache file. `chown -R` on both fixes it at the source, the
# same fix `/home/ci` already needed for the identical reason.
ARG CI_UID=1000
ARG CI_GID=1000
RUN groupadd --gid "${CI_GID}" ci \
    && useradd --uid "${CI_UID}" --gid "${CI_GID}" --create-home --shell /bin/bash ci \
    && mkdir -p /home/ci/.local/share/pnpm \
    && chown -R ci:ci /home/ci \
    && chown -R "${CI_UID}:${CI_GID}" /usr/local/cargo /usr/local/rustup

WORKDIR /repo

# So a bare `docker run` (bypassing compose.test.yaml's own `user:`
# entirely) is non-root too, by default — defense in depth, not the
# primary mechanism: compose.test.yaml's `user:` is still what actually
# controls the uid `just ci`/`ci-shell` run as, and still needs to match
# what this image was built with (`CI_UID`/`CI_GID` above) for the reasons
# that comment block already gives.
USER ci
