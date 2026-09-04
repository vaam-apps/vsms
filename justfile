# vsms — task runner.
#
# Expanding 16 models through `include_server_schema!` is memory-hungry enough
# to get rustc OOM-killed on a 32 GB machine at cargo's default job count. The
# recipes below cap concurrency rather than leaving each developer to discover
# that the hard way. See the `[profile.dev]` note in Cargo.toml.

# Cap build concurrency. Raise on a machine with headroom: `just jobs=8 check`,
# or export CARGO_BUILD_JOBS before invoking `just` (e.g. `CARGO_BUILD_JOBS=8
# just ci` — compose.test.yaml's own `runner` service reads that same env var
# through to the container, so this is also how `just ci`'s own concurrency
# gets raised). Read from the environment, not a bare literal, on purpose:
# `jobs := "4"` would have made every `_cargo`-prefixed command below emit a
# literal `CARGO_BUILD_JOBS=4` on argv, which — because an explicit
# command-line assignment wins over an already-exported environment variable
# of the same name — silently discarded any `CARGO_BUILD_JOBS` the caller had
# already set, the exact bug this comment exists to not reintroduce.
jobs := env_var_or_default("CARGO_BUILD_JOBS", "4")

_cargo := "CARGO_BUILD_JOBS=" + jobs + " cargo"

# `cratestack` binary used by client-gen/client-check (T3). Defaults to
# whatever `cratestack` resolves to on PATH; override for local dev with a
# pinned build, e.g. `just cratestack_bin=~/dev/cratestack/target/release/cratestack
# client-gen`. Requires >=0.7.8 — cratestack#456 fixed `Decimal` scalar TS
# emission (cratestack#455); anything older regenerates a client that fails
# to compile (`Cannot find name 'Decimal'`). Must match the pin read by
# `cargo xtask cratestack-pin` (read it; a version quoted in this comment
# has drifted before) — CI installs exactly that version via cratestack's
# own composite action (see ci.yml's `schema-drift`/`js` jobs), not a
# hardcoded literal here. `cargo xtask runner-pin` holds
# ci/runner/Dockerfile's own copy of the pin to the same value.
cratestack_bin := env_var_or_default("CRATESTACK_BIN", "cratestack")

# Show available recipes
help:
	@just --list

# Type-check the workspace
check:
	{{_cargo}} check --workspace --all-targets

# Run the test suite (unit + in-process — the 14 *_live_postgres.rs/*_live.rs
# suites stay #[ignore]d here; see `test-live`)
test:
	{{_cargo}} test --workspace

# Run every live-Postgres suite. `sms-test-support` brings up (or reuses)
# root compose.yml's `postgres` service, under its own reserved Compose
# project (`vsms-test-harness`), and applies both migrations — needs
# Docker, nothing else. Safe to rerun: the project+service name is fixed
# and Compose revives or reuses rather than recreates across runs.
#
# No `--tests` (removed for the cratestack 0.8.10 bump). CI's own
# `live-Postgres suites` job runs `cargo test --workspace -- --ignored`;
# this recipe is that command plus `--no-fail-fast`, and nothing else. The
# added flag only makes a local run report every failing suite instead of
# stopping at the first — strictly more information, and the direction
# AGENTS.md already argues for (workspace `cargo test` being fail-fast by
# default "is exactly how this stayed hidden behind whichever suite CI
# happened to reach first").
#
# What matters is that the target-kind restriction is gone, because that is
# the axis on which the two commands used to disagree: the 0.7.16 fix was
# verified with `--tests` locally while CI ran without it and went red. See
# the 0.7.16 bump section of AGENTS.md for that incident in full.
#
# `--tests` was added at 0.7.16 to dodge cratestack#512's generated
# `invoke_with_db` doc example, fenced ```` ```ignore ````, which a bare
# `-- --ignored` force-compiles (rustdoc reuses the same "ignored" bucket
# for "skipped test" and "non-compiling example") — 18 failures, one per
# procedure. That is fixed upstream in cratestack 0.8.6 (cratestack#611):
# the example is fenced ```` ```text ```` now, which rustdoc never schedules
# as a doctest under any flag. Confirmed live at 0.8.10, not assumed —
# `cargo test --workspace --no-fail-fast -- --ignored` runs clean, and
# `Doc-tests sms_api` reports `running 0 tests` rather than the pre-fix
# `18 ignored`. `backends/crates/sms-api`'s own `[lib] doctest = false`
# workaround was removed in the same change.
test-live:
	{{_cargo}} test --workspace --no-fail-fast -- --ignored

# Tear down the shared test-harness Postgres — container, network, and its
# named volume (so data doesn't linger as orphaned state). Scoped by the
# exact Compose project name (`vsms-test-harness`) `sms-test-support`
# itself reserves — never by image or a bare name pattern, so this can
# never touch a container this workspace's tests, or a developer's own
# `docker compose up` against the same compose.yml, did not create. Also
# removes a one-off pre-Compose leftover by its own exact legacy name, for
# a machine mid-migration off the old `docker run`-based harness. Not
# required for correctness (the harness self-heals on its own next run) —
# this is just for a developer who wants their machine back to zero now.
test-live-clean:
	docker rm -f vsms-test-harness-postgres 2>/dev/null || true
	docker compose -p vsms-test-harness -f compose.yml down --volumes --remove-orphans

# Format, then lint with warnings as errors
lint:
	cargo fmt --all --check
	{{_cargo}} clippy --workspace --all-targets -- -D warnings

# Apply formatting
fmt:
	cargo fmt --all

# Licence and advisory audit — every manifest CI's `deny` job checks, not
# just the root workspace. Before #324 this recipe covered only the root,
# silently out of step with CI's own SDK check (#234) and, since #324, the
# three workspace-`exclude`d manifests the root can never reach.
deny:
	cargo deny check
	cargo deny --manifest-path sdks/rust/vsms-sdk-rust/Cargo.toml check
	cargo deny --manifest-path ci/e2e-integration/vsms-e2e-integration/Cargo.toml check
	cargo deny --manifest-path examples/rust/sms-send/Cargo.toml check
	cargo deny --manifest-path deploy/backup-tool/Cargo.toml check

# The fast, host-toolchain subset — NOT the entire CI gate. It never runs
# `cargo deny`, the live-Postgres suites, or anything on the TypeScript
# side (all three need either Docker or a pnpm install this recipe assumes
# nothing about). `just ci` is the one command that runs everything CI
# runs; this is for iterating on one piece without paying for that.
# `sdk-schema-check`/`bootstrap-sql-check` need no external tool at all
# (plain text comparisons); `migrations-current` needs the `cratestack`
# CLI on PATH, version-locked to the pin — if it's missing or mismatched,
# the check fails with a readable message naming the mismatch, not a raw
# "command not found", so it's included here rather than left to `ci`.
# Run the fast, host-toolchain subset of checks — NOT the whole CI gate.
all-checks: lint test
	{{_cargo}} xtask no-raw-sqlx
	{{_cargo}} xtask parity
	{{_cargo}} xtask workflow-paths
	{{_cargo}} xtask runner-pin
	{{_cargo}} xtask docs-drift
	{{_cargo}} xtask r6
	{{_cargo}} xtask node-sdk-types-check
	{{_cargo}} xtask sdk-schema-check
	{{_cargo}} xtask bootstrap-sql-check
	{{_cargo}} xtask migrations-current

# #252: the hand-written Node SDK's enum unions must agree with schema.cstack
node-sdk-types-check:
	{{_cargo}} xtask node-sdk-types-check

# R2: the state diagram and the transition table must agree
parity:
	{{_cargo}} xtask parity

# R1: all data access goes through CrateStack delegates
no-raw-sqlx:
	{{_cargo}} xtask no-raw-sqlx

# Every path a workflow names must exist (release.yml never runs on a PR)
workflow-paths:
	{{_cargo}} xtask workflow-paths

# Every documentation path a doc, config file or runtime string names must
# exist. Catches a renamed runbook leaving a Prometheus alert annotation
# pointing at nothing — see .xtask/src/docs_drift.rs for the incident list.
docs-drift:
	{{_cargo}} xtask docs-drift

# ci/runner/Dockerfile's CRATESTACK_VERSION default is a second copy of the
# Cargo.toml pin; it drifted once (0.8.10 against a 0.11.0 pin) and just ci
# built a runner its own first step then refused.
runner-pin:
	{{_cargo}} xtask runner-pin
# R6: no CSS classes or raw markup in page/*-screen view files
r6:
	{{_cargo}} xtask r6

# The Rust SDK's vendored schema.cstack must match schemas/vsms.cstack
sdk-schema-check:
	{{_cargo}} xtask sdk-schema-check

# 0001_init must match what `cratestack migrate diff` produces from the
# current schema.cstack. Needs a `cratestack` CLI on PATH, version-locked
# to the pin `cargo xtask cratestack-pin` reads from Cargo.toml.
migrations-current:
	{{_cargo}} xtask migrations-current

# Print the generated route table. Needs no database.
routes:
	{{_cargo}} run -p sms-gateway -- routes

# Apply migrations to a scratch database, run the state-machine assertions,
# drop it. `ci/apply-migrations.sh` is gone (containerize-tooling PR) —
# `app/sms-migrate` (already a real workspace member) is its direct Rust
# replacement, run here the same way `cargo xtask migrations-current`
# already runs other workspace tools directly rather than through a shell
# wrapper. `createdb`/`dropdb`/`psql` themselves stay host-native CLI
# calls against a reachable Postgres (`compose.yml`'s own, or a local
# install) — a pre-existing pattern this recipe already used, not one of
# the seven scripts that PR converted.
schema-check:
	createdb vsms_check
	DATABASE_URL=postgres://localhost/vsms_check {{_cargo}} run -q -p sms-migrate
	psql postgres://localhost/vsms_check -v ON_ERROR_STOP=1 -f ci/test-state-machine.sql
	dropdb vsms_check

# Merge docs/architecture.md, the runbooks, CONTRIBUTING.md and friends into
# one PDF book. pandoc + Typst both run inside a single pinned container
# image (never installed on the host) — needs `docker`, nothing else. See
# `.xtask/src/docs_pdf.rs` for the pipeline and its own design notes.
docs-pdf:
	{{_cargo}} xtask docs-pdf

# Regenerate 0002_bootstrap from §2.10 of the design doc
bootstrap-sql:
	{{_cargo}} xtask bootstrap-sql backends/migrations/postgres/0002_bootstrap/up.sql

# Regenerate frontends/packages/sms-client from schemas/vsms.cstack (T3). Per the
# owner's standing rule, generated code is never committed — this package
# is gitignored except `package.json` (see the .gitignore comment there
# and frontends/packages/sms-client/GENERATING.md) and must be regenerated after
# every `pnpm install`, before anything (`tsc`, `turbo`, `pnpm run build`)
# consumes it. `--base-path ''` is load-bearing — the server serves routes
# at `/`, not the generator's `/api` default. Also applies the DO-NOT-EDIT
# README banner (see ci/postprocess-sms-client-readme.mjs) as a
# deterministic, reproducible post-processing step, not a one-off hand-edit.
#
# `--tanstack` is load-bearing for the same reason `--base-path ''` is, and
# it is new as of the cratestack 0.8.3 bump. Through 0.8.0 the generator
# emitted `src/react-query.ts` plus the `@tanstack/react-query` peer and dev
# dependencies unconditionally; cratestack#617 (in 0.8.1) gated all three
# behind this additive flag, finishing the same convergence `--swr` (#589)
# and `--refine` (#571) already went through. The tracked
# `frontends/packages/sms-client/package.json` — the one file in that
# otherwise-gitignored package that IS committed — declares both of those
# dependencies, so without this flag a regeneration silently rewrites it
# and the tracked file drifts from what the generator produces. Verified
# byte-for-byte at 0.8.3: with `--tanstack` the emitted `package.json` is
# identical to the committed one; without it, it differs by exactly those
# two lines. Dropping the tanstack deps instead is defensible — nothing in
# this repo imports `@vsms/sms-client` at runtime yet (checked repo-wide,
# and `frontends/apps/admin/Dockerfile` records the same finding) — but
# that is a deliberate scope decision about the package's shape, not
# something a dependency bump should make on its own.
client-gen:
	{{cratestack_bin}} generate-typescript --schema schemas/vsms.cstack \
		--out frontends/packages/sms-client --package-name @vsms/sms-client --base-path '' \
		--tanstack
	node ci/postprocess-sms-client-readme.mjs frontends/packages/sms-client/README.md

# The drift gate over frontends/packages/sms-client that still means something once
# nothing is committed (T3). There used to be a second gate here —
# "does the committed client match schemas/vsms.cstack?" — but with no
# committed client there is nothing for it to diff against; it would
# assert nothing, so it was removed rather than kept as decoration.
#
# What remains, and matters more than that removed gate ever did: does
# every route the freshly generated client calls exist on the pinned
# server's real route table? The client can be generated by any
# `cratestack` CLI version, but `sms-gateway` is built from the pinned
# library family (currently =0.8.10 — read it with `cargo xtask
# cratestack-pin` rather than trusting this comment, which has drifted
# before: it read a stale "=0.6.7" since the 0.6.7-era pin, uncaught until
# the 0.7.16 bump grepped for every version literal in the repo), and
# AGENTS.md already
# documents these diverging for `migrate diff` on this machine. A
# CLI/library route-shape skew would 404 in production and nothing else
# here would catch it.
client-check: client-gen
	{{_cargo}} build -p sms-gateway
	node ci/assert-client-routes-match-server.mjs

# Bring up the full demo chain — scratch Postgres, both migrations, an OP
# signing key, an `App`+`Provider`+`Route`+`SenderId`, a machine client,
# the `sms-console` OIDC client, a human operator account, sms-fake-orange,
# sms-gateway, sms-worker (dispatch,scheduler,jobs), and the admin console
# — as containers, built from this checkout's own source
# (`compose.dev.yaml`; see that file's own header for the full design and
# why it doesn't reuse `compose.yml`). NOT for production: sms-fake-orange
# impersonates Orange Cameroon's API and sends no real SMS. See
# docs/runbooks/local-development.adoc for what this brings up and why.
#
# `down -v` first, every time — not just on request: `provision-client`
# (inside `compose.dev.yaml`) refuses to overwrite an existing private
# key, so a second `up` against the same named volumes would otherwise
# fail loudly on that step rather than silently reusing stale credentials.
# This is the compose-native equivalent of `scripts/demo.sh`'s own former
# "reset the database on every up" behaviour — a full volume wipe rather
# than a targeted `DROP DATABASE`/`CREATE DATABASE`.
#
# The build step is forced strictly sequential, one distinct image at a
# time — found live, not assumed: every `app/*/Dockerfile` builder stage
# deliberately shares one BuildKit cache-mount id (`cargo-registry-musl`)
# across sms-gateway/sms-worker/sms-fake-orange/sms-migrate, "so building
# any one warms the cache for the other three" (their own comment).
# That's true for *sequential* builds; building more than one for the
# first time at once races several `cargo` processes extracting into the
# *same* registry cache directory concurrently and reproduces a real,
# non-deterministic failure (`failed to unpack package <whichever crate
# lost the race>: File exists (os error 17)`) — seen on at least two
# separate machines, a different crate each time (`pem`, then `pkcs1`).
#
# `COMPOSE_PARALLEL_LIMIT=1` alone was the original fix and stopped being
# reliable the moment a machine's Docker Compose defaults `build` to
# `buildx bake` (Compose >=2.x with a recent buildx): bake fans every
# target out into one concurrent invocation regardless of that env var,
# which only ever throttled the legacy per-service build loop. Confirmed
# live: the race reproduced again with `COMPOSE_PARALLEL_LIMIT=1` set
# exactly as before, and `COMPOSE_BAKE=false` on its own wasn't
# sufficient either (a single `build` invocation with both env vars set
# still launched multiple images' `cargo build`s concurrently). What
# actually serializes it, on every toolchain: genuinely separate,
# blocking `docker compose build <service>` invocations, one at a time —
# so below.
#
# Seven loop entries, not all fifteen `--profile console` services: the
# other eight (`provision-client`, `provision-user`, `seed-console-client`,
# `seed-dispatch`, `seed-signing-key`, plus non-building services) either
# share `sms-gateway`'s own explicit `image: vsms-dev/sms-gateway:local`
# tag (building `sms-gateway` once already produces the image every one
# of those needs — confirmed via `docker compose ... config --format
# json`) or build nothing at all. `demo-app` joined this list as its own,
# image-distinct entry (a genuinely separate Dockerfile, `examples/node/
# demo-app/Dockerfile` — Node, not Rust, so it never shares the cargo
# cache-mount ids the race above is actually about, but the sequential
# loop costs it nothing and keeps this list's own reasoning uniform
# rather than special-casing one entry). This list can drift if a *new*,
# image-distinct build target is ever added to `compose.dev.yaml` without
# a matching entry here — re-derive it with the `config --format json`
# query above if a build ever silently skips a service's own image.
#
# `down -v` first, every time — not just on request: `provision-client`
# (inside `compose.dev.yaml`) refuses to overwrite an existing private
# key, so a second `up` against the same named volumes would otherwise
# fail loudly on that step rather than silently reusing stale credentials.
# This is the compose-native equivalent of `scripts/demo.sh`'s own former
# "reset the database on every up" behaviour — a full volume wipe rather
# than a targeted `DROP DATABASE`/`CREATE DATABASE`. Note `down -v` wipes
# named *volumes* only, not the BuildKit cache — a cache already warmed
# by a previous `just demo` doesn't hit the race above (nothing new to
# extract), so this mainly costs time on the very first run.
#
# `up -d` — deliberately WITHOUT `--wait`, and as a SINGLE invocation for
# the whole `--profile console` set. Two real bugs were found live,
# in this order, getting here:
#
# 1. Plain `up -d --wait` (no service names) makes `--wait` poll
#    `demo-app` too, and `demo-app` is a one-shot evaluator
#    (`restart: 'no'`, no healthcheck) that can genuinely finish — with
#    exit 0, a real SUCCESS — before `--wait`'s own convergence check
#    next runs. The instant it observes an Exited container in its wait
#    set, Compose fails the whole `up -d --wait` invocation (exit 1),
#    regardless of that container's own exit code. Reproduced on a real
#    run: `demo-app` printed a genuine `SUCCESS: ... reached delivered
#    with 2 verified webhook(s)`, and `just demo` still reported
#    failure, because `up -d --wait`'s own line failed before the `wait
#    demo-app`/`logs demo-app` lines below it ever ran.
#
# 2. The first fix for (1) split this into TWO separate `docker compose
#    up` invocations — `up -d --wait <the four long-running services>`,
#    then a second `up -d demo-app` to actually start it. That is worse,
#    not better: two independent CLI processes against the same project,
#    started moments apart, do not share one atomic view of "has this
#    one-shot dependency already run" — reproduced live, not assumed:
#    the second `up -d demo-app` call (demo-app depends on
#    `secrets-fix-perms`, which depends on `provision-client`) caused
#    Compose to recreate and RE-RUN `provision-client` a second time,
#    even though the first `up` call had already run it to a clean
#    success moments earlier. `provision-client` refuses to overwrite an
#    existing private key (by design — see that command's own doc), so
#    the second run's own log shows the bizarre-looking shape of a
#    genuine success (a real `provisioned client:`/`private key written
#    to:` — a SECOND, real `AppClient` row, a real side effect) followed
#    immediately by `Error: ... already exists — refusing to overwrite`,
#    and the whole recipe failed on a completely different line.
#
# The actual fix needs neither trick: `depends_on: condition:
# service_healthy`/`service_completed_successfully` (already correct,
# already how every other one-shot step in this file gets sequenced)
# blocks a dependent container's own START regardless of `--wait` —
# `--wait` only ever added "also block the CLI and report readiness",
# which this recipe doesn't need from `up` itself, because the very next
# line (`docker compose wait demo-app`) already blocks for real
# completion and hands back demo-app's own real exit code. One plain
# `up -d`, one Compose invocation, the same dependency graph deciding
# everything exactly as it already did for migrate/seed-signing-key/
# seed-dispatch/provision-client/etc. — no race between two CLI
# processes, and no service whose "Exited" state `--wait` could
# misread as a failure.
#
# `--profile console --profile demo`, everywhere in this section (not
# just `--profile console`): `demo-app` and its own `seed-demo-webhook`
# dependency moved to a dedicated `demo` profile, deliberately never
# `console` — R4 (CONTRIBUTING.md): `demo-app` is a *backend* proof (it
# talks to `sms-gateway` directly, never through the console), so it has
# to be reachable without ever starting `admin`, and a genuinely
# backend-only `up -d` (no profiles at all) must not seed a
# `WebhookEndpoint` pointed at a container that will never exist. See
# `compose.dev.yaml`'s own `demo-app`/`seed-demo-webhook`/
# `secrets-fix-perms` comments for the full mechanism; `docker compose
# --profile demo config --services` (no `console`) lists `demo-app` and
# not `admin`, confirming the split holds on its own, not just combined.
#
# `demo-up` is split out from `demo` (below) specifically so
# `e2e-integration` can depend on "the stack is up" without also
# inheriting `demo`'s own "then wait for demo-app and propagate its exit
# code" behaviour — `e2e-integration` doesn't touch `demo-app` at all,
# and a `demo-app` failure (a real one, or a flaky run) has nothing to
# do with what `e2e-integration` is trying to prove.
demo-up:
	docker compose -f compose.dev.yaml --profile console --profile demo down -v --remove-orphans
	for svc in sms-gateway migrate seed-demo-app sms-fake-orange sms-worker admin demo-app; do \
		COMPOSE_BAKE=false docker compose -f compose.dev.yaml --profile console --profile demo build "$svc"; \
	done
	docker compose -f compose.dev.yaml --profile console --profile demo up -d

# `; demo_exit=$?; ... ; exit $demo_exit`, not `&&`/separate recipe
# lines: `just` aborts a recipe the moment any line exits non-zero,
# which would skip the `logs demo-app` line entirely on the one run
# where seeing those logs matters most — a failed demo.
#
# `up -d` (inside `demo-up`) is deliberately WITHOUT `--wait`, and as a
# SINGLE invocation for the whole profile set. Two real bugs were found
# live, in this order, getting here:
#
# 1. Plain `up -d --wait` (no service names) makes `--wait` poll
#    `demo-app` too, and `demo-app` is a one-shot evaluator
#    (`restart: 'no'`) that can genuinely finish — with exit 0, a real
#    SUCCESS — before `--wait`'s own convergence check next runs. The
#    instant it observes an Exited container in its wait set, Compose
#    fails the whole `up -d --wait` invocation (exit 1), regardless of
#    that container's own exit code. Reproduced on a real run: `demo-app`
#    printed a genuine `SUCCESS: ... reached delivered with 2 verified
#    webhook(s)`, and `just demo` still reported failure, because `up -d
#    --wait`'s own line failed before the `wait demo-app`/`logs demo-app`
#    lines below it ever ran.
#
# 2. The first fix for (1) split this into TWO separate `docker compose
#    up` invocations — `up -d --wait <the four long-running services>`,
#    then a second `up -d demo-app` to actually start it. That is worse,
#    not better: two independent CLI processes against the same project,
#    started moments apart, do not share one atomic view of "has this
#    one-shot dependency already run" — reproduced live, not assumed:
#    the second `up -d demo-app` call (demo-app depends on
#    `secrets-fix-perms`, which depends on `provision-client`) caused
#    Compose to recreate and RE-RUN `provision-client` a second time,
#    even though the first `up` call had already run it to a clean
#    success moments earlier. `provision-client` refuses to overwrite an
#    existing private key (by design — see that command's own doc), so
#    the second run's own log shows the bizarre-looking shape of a
#    genuine success (a real `provisioned client:`/`private key written
#    to:` — a SECOND, real `AppClient` row, a real side effect) followed
#    immediately by `Error: ... already exists — refusing to overwrite`,
#    and the whole recipe failed on a completely different line.
#
# The actual fix needs neither trick: `depends_on: condition:
# service_healthy`/`service_completed_successfully` (already correct,
# already how every other one-shot step in this file gets sequenced)
# blocks a dependent container's own START regardless of `--wait` —
# `--wait` only ever added "also block the CLI and report readiness",
# which this recipe doesn't need from `up` itself, because the very next
# line (`docker compose wait demo-app`) already blocks for real
# completion and hands back demo-app's own real exit code. One plain
# `up -d`, one Compose invocation, the same dependency graph deciding
# everything exactly as it already did for migrate/seed-signing-key/
# seed-dispatch/provision-client/etc. — no race between two CLI
# processes, and no service whose "Exited" state `--wait` could
# misread as a failure.
#
# No `--timeout` on `docker compose wait` below — checked, not assumed:
# `docker compose wait --help` on this VM's Compose (v5.5.1) lists only
# `--down-project`/`--dry-run`, no timeout flag. The real bound is
# `demo-app`'s own `DEMO_TIMEOUT_MS` (default 90s — see its own README),
# which the process enforces internally and always exits on, one way or
# the other; `wait` here just blocks for however long that takes.
demo: demo-up
	docker compose -f compose.dev.yaml --profile console --profile demo wait demo-app; demo_exit=$?; \
	docker compose -f compose.dev.yaml --profile console --profile demo logs demo-app; \
	exit $demo_exit

# Stop everything `just demo`/`demo-up` started and remove its volumes
# (scratch Postgres data, provisioned secrets) — by exact Compose project
# name (`vsms-dev`) only, never touching `compose.yml`'s own `vsms`
# project or anything unrelated on the machine.
demo-down:
	docker compose -f compose.dev.yaml --profile console --profile demo down -v --remove-orphans

# What's currently running from `just demo`.
demo-status:
	docker compose -f compose.dev.yaml --profile console --profile demo ps

# The generated password `just demo` provisioned for `demo@vsms.local` —
# printed once, to `provision-user`'s own container log, never stored
# anywhere (see backends/apps/sms-gateway/src/main.rs's own `ProvisionUser` doc).
demo-login:
	docker compose -f compose.dev.yaml logs provision-user

# Re-run `demo-app` (`examples/node/demo-app`) against an ALREADY-RUNNING
# `just demo` stack, without tearing anything else down — the fast loop
# for iterating on the evaluator itself (or re-proving the end-to-end
# story after `sendMessage` traffic has already flowed once) rather than
# a full `just demo`. `--no-deps`: everything `demo-app` depends on is
# already up; re-resolving dependencies here would at best be a no-op and
# at worst race `seed-demo-app`'s own one-shot `restart: 'no'` container
# into trying to run again. `--force-recreate`: a bare `up -d` would
# no-op on a service whose image/config hasn't changed, even though the
# whole point of rerunning is a fresh attempt (a new message, a fresh
# webhook exchange) — recreating is what actually restarts it.
demo-app:
	docker compose -f compose.dev.yaml --profile console --profile demo build demo-app
	docker compose -f compose.dev.yaml --profile console --profile demo up -d --no-deps --force-recreate demo-app
	docker compose -f compose.dev.yaml --profile console --profile demo wait demo-app; demo_exit=$?; \
	docker compose -f compose.dev.yaml --profile console --profile demo logs demo-app; \
	exit $demo_exit

# #160: the joined integration story — brings up the stack (`demo-up`,
# not `demo`: this recipe never touches `demo-app` and doesn't want a
# `demo-app` outcome, real or flaky, deciding its own exit code),
# provisions a SECOND client against the same App ("external integrator"),
# then runs `ci/e2e-integration` (a small Rust tool, not a bash script —
# see its own module doc for why it has to run *inside* the Compose
# network rather than as a host process) to send as that integrator over
# real HTTP and poll GET /messages/{id} AS THE CONSOLE's own credential —
# the same route frontends/packages/gateway/src/messages.ts's getMessageById calls —
# until that exact message id reaches `delivered`. Fails loudly (non-zero
# exit) if any link in the chain breaks. See docs/runbooks/e2e-integration.adoc
# for what this proves, what it fakes (Orange, via sms-fake-orange — #36's
# handset gate is unaffected), and why both clients share one App.
#
# `.e2e/` (gitignored) is where the integrator's own key/id land on the
# host — `docker compose run --rm` removes its container immediately, so
# there is no `docker compose cp` source for it the way `provision-client`'s
# own long-lived container (from `up`, still present) is for the console's
# credential two lines below.
e2e-integration: demo-up
	mkdir -p .e2e
	rm -f .e2e/integrator-key.pem .e2e/integrator-client-id
	docker compose -f compose.dev.yaml run --rm -v "{{justfile_directory()}}/.e2e:/out" sms-gateway \
		provision-client --app-slug vsms-demo --label "external integrator (e2e-integration)" \
		--scope sms:send --scope sms:read \
		--key-out /out/integrator-key.pem --client-id-out /out/integrator-client-id
	docker compose -f compose.dev.yaml cp provision-client:/secrets/console-client-key.pem .e2e/console-client-key.pem
	docker compose -f compose.dev.yaml cp provision-client:/secrets/console-client-id .e2e/console-client-id
	docker build -f ci/e2e-integration/Dockerfile -t vsms-e2e-integration:local .
	docker run --rm --network vsms-dev_default -v "{{justfile_directory()}}/.e2e:/secrets:ro" vsms-e2e-integration:local \
		--gateway-url http://sms-gateway:8080 \
		--integrator-client-id "$(cat .e2e/integrator-client-id)" \
		--integrator-key-path /secrets/integrator-key.pem \
		--console-client-id "$(cat .e2e/console-client-id)" \
		--console-key-path /secrets/console-client-key.pem

# ---------------------------------------------------------------------------
# `just ci` — the entire CI gate, in one command, with nothing installed on
# the host but `docker`, `docker compose` and `just`. See
# docs/runbooks/testing.adoc for the guided walkthrough.
#
# `ci-inner` mirrors .github/workflows/ci.yml's own eight jobs, in the same
# order, run inside `compose.test.yaml`'s `runner` container against its
# own disposable `postgres` service — not a reimplementation of what CI
# runs, the same commands, so "passes locally" and "passes in CI" are the
# same claim. It is NOT meant to be run directly on a bare host: it assumes
# the runner image's toolchain (pinned cratestack CLI, Node 26, psql 16,
# `just`, `cargo-deny`) and `VSMS_TEST_DATABASE_URL` pointing at a reachable
# Postgres — both of which only exist inside the container `just ci` builds.

# Build the runner image (also done implicitly by `just ci`/`ci-shell`).
ci-build:
	docker compose -f compose.test.yaml build runner

# Run the whole CI gate inside the container. First run is slow (a full
# toolchain image build, then a cold cargo/pnpm fetch) — subsequent runs
# reuse the named volumes (cargo registry, pnpm store, target dir) and are
# much faster. `--build` keeps the image current with ci/runner/Dockerfile
# without needing a separate `ci-build` step first.
# Run the entire CI gate (all 22 steps) inside the container.
ci:
	#!/usr/bin/env bash
	set -euo pipefail
	if [ -f .git ]; then
		echo "error: .git is a file, not a directory — this looks like a linked git" >&2
		echo "worktree (\`git worktree add\`). \`just ci\` bind-mounts only this directory" >&2
		echo "into the container; a linked worktree's .git redirect points at an" >&2
		echo "absolute host path OUTSIDE it, which the container can't see — cargo" >&2
		echo "xtask docs-drift's \`git ls-files\` and Turborepo's own root detection" >&2
		echo "both break on it. See docs/runbooks/testing.adoc's Troubleshooting" >&2
		echo "section for the fix (a plain \`git clone\`, or two extra bind mounts)." >&2
		exit 2
	fi
	docker compose -f compose.test.yaml run --build --rm runner just ci-inner

# The fast-iteration subset: everything in `ci` except the live-Postgres
# suites (steps 13, the slowest single step by a wide margin) and the JS
# typecheck+build+test (steps 17-18, skipped together — turbo.json's own
# "test" task depends on "build", so skipping only the build step saves
# nothing; `pnpm turbo run test` would just trigger the identical build as
# one of its own dependency tasks). Good for "did I break something
# obvious" before paying for the full gate.
# Run everything in `ci` except the live-Postgres suites and JS build/test.
ci-quick:
	#!/usr/bin/env bash
	set -euo pipefail
	if [ -f .git ]; then
		echo "error: .git is a file, not a directory — this looks like a linked git" >&2
		echo "worktree (\`git worktree add\`). \`just ci\`/\`ci-quick\` bind-mount only this" >&2
		echo "directory into the container; a linked worktree's .git redirect points" >&2
		echo "at an absolute host path OUTSIDE it, which the container can't see —" >&2
		echo "cargo xtask docs-drift's \`git ls-files\` and Turborepo's own root" >&2
		echo "detection both break on it. See docs/runbooks/testing.adoc's" >&2
		echo "Troubleshooting section for the fix (a plain \`git clone\`, or two" >&2
		echo "extra bind mounts)." >&2
		exit 2
	fi
	docker compose -f compose.test.yaml run --build --rm -e VSMS_CI_QUICK=1 runner just ci-inner

# Drop into an interactive shell in the runner container — same image,
# same mounted repo and volumes, same database reachable at
# postgres:5432 — for running one suite by hand:
#   just ci-shell
#   cargo test -p sms-worker --no-fail-fast -- --ignored
# Open an interactive shell in the runner container.
ci-shell:
	docker compose -f compose.test.yaml run --build --rm runner bash

# Tear down the CI stack: containers, network, and every named volume
# (Postgres data, cargo registry/git caches, pnpm store, target dir) —
# scoped to this file's own Compose project (`vsms-ci`) only, never
# touching `compose.yml`'s `vsms` project, `compose.dev.yaml`'s `vsms-dev`,
# or `sms-test-support`'s own `vsms-test-harness`.
# Remove the CI stack's containers, network and named volumes.
ci-clean:
	docker compose -f compose.test.yaml down --volumes --remove-orphans

# The actual gate script, run INSIDE the container by `ci`/`ci-quick` above
# — never call this directly on a bare host, it assumes the runner image's
# toolchain and `VSMS_TEST_DATABASE_URL`. Set VSMS_CI_QUICK=1 (as `ci-quick`
# does) to skip the live-Postgres suites (step 13) and the JS
# typecheck+build+test steps (17-18, together — see `ci-quick`'s own
# comment for why splitting them saves nothing).
# The 22-step gate script itself — run inside the container, not directly.
ci-inner:
	#!/usr/bin/env bash
	set -euo pipefail
	step() { echo; echo "=== STEP $1/$2: $3 ==="; }
	quick="${VSMS_CI_QUICK:-}"

	step 1 22 "cratestack CLI matches the pin"
	pinned="$(cargo xtask cratestack-pin)"
	installed="$(cratestack --version | awk '{print $2}')"
	if [ "$pinned" != "$installed" ]; then
		echo "cratestack CLI ($installed) does not match the pin ($pinned)." >&2
		echo "Rebuild the runner image: docker compose -f compose.test.yaml build --build-arg CRATESTACK_VERSION=$pinned runner" >&2
		exit 1
	fi
	echo "cratestack $installed matches the pin"

	step 2 22 "lint (fmt --check, clippy -D warnings)"
	just lint

	step 3 22 "cargo test --workspace (unit + in-process)"
	{{_cargo}} test --workspace

	step 4 22 "Cargo.lock is up to date"
	cargo metadata --locked --format-version 1 > /dev/null

	step 5 22 "Rust SDK (vsms-sdk-rust) — check, clippy, test, aws-lc-rs absence, publish dry-run"
	# --allow-dirty: this recipe runs against whatever the caller's actual
	# working tree looks like, which — being a local run, not a CI
	# checkout — is routinely mid-PR-review or otherwise not committed.
	# ci.yml's own `rust` job keeps the strict form (no --allow-dirty),
	# since it always runs on a clean checkout and a dirty tree there
	# would mean something genuinely wrong with the checkout step itself.
	( cd sdks/rust/vsms-sdk-rust \
	  && cargo check --locked --all-targets \
	  && cargo clippy --all-targets -- -D warnings \
	  && cargo test \
	  && ( cargo tree -i aws-lc-rs && exit 1 || true ) \
	  && cargo publish --dry-run --allow-dirty )

	step 6 22 "Examples (Rust) — check, clippy"
	( cd examples/rust && cargo check --all-targets && cargo clippy --all-targets -- -D warnings )

	step 7 22 "cargo deny — advisories, bans, licenses, sources (all five manifests)"
	just deny

	step 8 22 "R1/R2/R6 and drift guards (cargo xtask)"
	{{_cargo}} xtask no-raw-sqlx
	{{_cargo}} xtask parity
	{{_cargo}} xtask sdk-schema-check
	{{_cargo}} xtask workflow-paths
	{{_cargo}} xtask runner-pin
	{{_cargo}} xtask docs-drift
	{{_cargo}} xtask r6
	{{_cargo}} xtask node-sdk-types-check

	step 9 22 "0001_init matches \`cratestack migrate diff\`"
	{{_cargo}} xtask migrations-current

	step 10 22 "0002_bootstrap matches the design doc"
	{{_cargo}} xtask bootstrap-sql-check

	step 11 22 "Apply migrations to a fresh scratch database, then the state-machine SQL assertions"
	# A fresh, per-run scratch database, not the persistent `vsms_ci_pgdata`
	# volume's own `vsms` database — found in review, and real: that volume
	# survives across `just ci` runs (it's the whole point of caching it),
	# so from the second run on, applying migrations against the SAME
	# already-migrated database makes `sms-migrate` log "already applied —
	# skipping" for all three and do nothing at all. A regenerated
	# 0001_init with a genuine psql-time error (a column type CI's own
	# `migrations` job — a real fresh-container `postgres:16` service, every
	# single run — would catch) would then pass `just ci` silently. The
	# name carries a timestamp plus a random suffix rather than `$$`: a PID
	# inside a container's own PID namespace is low and deterministic
	# (two concurrent runners both got `1` — found in review), so two
	# overlapping `ci`/`ci-quick` runs against the shared `postgres`
	# service would have collided on the same name. The `trap` guarantees
	# the scratch database is dropped even if `sms-migrate` or the SQL
	# assertions fail partway through, not just on the happy path.
	scratch_db="vsms_ci_migrate_$(date +%s)_${RANDOM}"
	cleanup_scratch_db() { dropdb -h postgres -U vsms --if-exists "$scratch_db" >/dev/null 2>&1 || true; }
	trap cleanup_scratch_db EXIT
	createdb -h postgres -U vsms "$scratch_db"
	DATABASE_URL="postgres://vsms:vsms@postgres:5432/${scratch_db}" cargo run -p sms-migrate
	psql "postgres://vsms:vsms@postgres:5432/${scratch_db}" -v ON_ERROR_STOP=1 -q -f ci/test-state-machine.sql
	cleanup_scratch_db
	trap - EXIT

	step 12 22 "Sample Node receiver's dependencies (for the live gate suite)"
	( cd examples/node/webhook-receiver && pnpm install --ignore-workspace --frozen-lockfile )

	if [ -n "$quick" ]; then
		echo; echo "VSMS_CI_QUICK set — skipping the live-Postgres suites (step 13)."
	else
		step 13 22 "Live-Postgres suites (sms-test-support, against this container's own postgres)"
		{{_cargo}} test --workspace --no-fail-fast -- --ignored
	fi

	step 14 22 "pnpm install (workspace)"
	pnpm install --frozen-lockfile

	step 15 22 "Biome (format + lint)"
	pnpm biome ci .

	step 16 22 "Generate the client and check its routes against the server"
	just client-check

	# Steps 17 and 18 are skipped together under VSMS_CI_QUICK, not
	# independently — found in review: turbo.json's own "test" task
	# `dependsOn: ["build"]`, so skipping only step 17 saves nothing at
	# all. `pnpm turbo run test` would simply trigger the identical build
	# as one of its own dependency tasks, just folded silently into step
	# 18's own timing instead of appearing as step 17's.
	if [ -n "$quick" ]; then
		echo; echo "VSMS_CI_QUICK set — skipping JS typecheck+build and tests (steps 17-18)."
	else
		step 17 22 "Typecheck and build (pnpm turbo)"
		pnpm turbo run typecheck build

		step 18 22 "Tests (pnpm turbo)"
		pnpm turbo run test
	fi

	step 19 22 "Sample Node receiver — cross-language signature vectors"
	( cd examples/node/webhook-receiver && node --test )

	# examples/node/demo-app carries a byte-for-byte copy of
	# signature.ts/cross-language-vectors.test.ts from the webhook-receiver
	# step just above (per that package's own README) — gated the same way,
	# plus a typecheck, mirroring ci.yml's "js" job step-for-step. `tsc` is
	# invoked from node_modules directly rather than via `pnpm run
	# typecheck`: pnpm verifies dependencies before every `run`, and from a
	# directory under the repo root — without --ignore-workspace — that
	# check resolves against the ROOT workspace lockfile, which knows
	# nothing about this standalone package, and fails with
	# ERR_PNPM_OUTDATED_LOCKFILE.
	step 20 22 "Demo app — typecheck and cross-language signature vectors"
	( cd examples/node/demo-app && pnpm install --ignore-workspace --frozen-lockfile && node_modules/.bin/tsc --noEmit && node --test )

	step 21 22 "Official Node SDK (@vymalo/vsms-node) — build, typecheck, test, pack dry-run"
	pnpm --filter @vymalo/vsms-node run build
	pnpm --filter @vymalo/vsms-node run typecheck
	( cd sdks/node/vsms-sdk-node && node --test && npm pack --dry-run )

	step 22 22 "Mermaid diagrams parse (no browser)"
	( cd ci/mermaid-parse && npm ci )
	node ci/mermaid-parse/parse.mjs docs/architecture.md
	node ci/mermaid-parse/parse.mjs docs/roadmap.md

	echo
	echo "=== just ci: all steps passed ==="
