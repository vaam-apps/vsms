# vsms — task runner.
#
# Expanding 16 models through `include_server_schema!` is memory-hungry enough
# to get rustc OOM-killed on a 32 GB machine at cargo's default job count. The
# recipes below cap concurrency rather than leaving each developer to discover
# that the hard way. See the `[profile.dev]` note in Cargo.toml.

# Cap build concurrency. Raise on a machine with headroom: `just jobs=8 check`.
jobs := "4"

_cargo := "CARGO_BUILD_JOBS=" + jobs + " cargo"

# `cratestack` binary used by client-gen/client-check (T3). Defaults to
# whatever `cratestack` resolves to on PATH; override for local dev with a
# pinned build, e.g. `just cratestack_bin=~/dev/cratestack/target/release/cratestack
# client-gen`. Requires >=0.7.8 — cratestack#456 fixed `Decimal` scalar TS
# emission (cratestack#455); anything older regenerates a client that fails
# to compile (`Cannot find name 'Decimal'`). CI installs the published
# 0.7.8 from crates.io onto PATH, so the default is correct there.
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
# `--tests` (added for the cratestack 0.7.16 bump): every live-Postgres
# suite lives under a `tests/` integration-test binary — never a doctest,
# never a `--lib` unit test — so restricting to that target kind loses no
# real coverage. Needed because cratestack 0.7.13 (cratestack#512) added a
# generated `invoke_with_db` doc comment to every procedure module, with an
# illustrative, deliberately non-compiling pseudocode example fenced
# ```` ```ignore ````. `cargo test`'s doctest runner treats `--ignored` as
# "actually try to compile and run the ones marked ignore" — the opposite
# of what `--ignored` does for `#[ignore]`-attributed tests — so a bare
# `cargo test --workspace -- --ignored` now genuinely tries to compile that
# pseudocode once per procedure (18 failures, all `cannot find value/type`
# for names the example never defines, e.g. `SystemContext`/`registry`) and
# fails the whole run. Confirmed live: `cargo test --workspace --no-fail-fast
# -- --ignored` (no `--tests`) reproduces exactly this; adding `--tests`
# restores a clean run with identical real coverage.
test-live:
	{{_cargo}} test --workspace --tests -- --ignored

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

# Licence and advisory audit
deny:
	cargo deny check

# Everything CI runs, in CI's order
all-checks: lint test
	{{_cargo}} xtask no-raw-sqlx
	{{_cargo}} xtask parity
	{{_cargo}} xtask workflow-paths
	{{_cargo}} xtask r6

# R2: the state diagram and the transition table must agree
parity:
	{{_cargo}} xtask parity

# R1: all data access goes through CrateStack delegates
no-raw-sqlx:
	{{_cargo}} xtask no-raw-sqlx

# Every path a workflow names must exist (release.yml never runs on a PR)
workflow-paths:
	{{_cargo}} xtask workflow-paths
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
client-gen:
	{{cratestack_bin}} generate-typescript --schema schemas/vsms.cstack \
		--out frontends/packages/sms-client --package-name @vsms/sms-client --base-path ''
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
# library family (currently =0.7.16 — this comment had drifted to a stale
# "=0.6.7" since the 0.6.7-era pin, never caught until the 0.7.16 bump
# grepped for every version literal in the repo), and AGENTS.md already
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
# docs/runbooks/local-development.md for what this brings up and why.
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
# Six loop entries, not all fourteen `--profile console` services: the
# other eight (`provision-client`, `provision-user`, `seed-console-client`,
# `seed-dispatch`, `seed-signing-key`, plus non-building services) either
# share `sms-gateway`'s own explicit `image: vsms-dev/sms-gateway:local`
# tag (building `sms-gateway` once already produces the image every one
# of those needs — confirmed via `docker compose ... config --format
# json`) or build nothing at all. This list can drift if a *new*, image-
# distinct build target is ever added to `compose.dev.yaml` without a
# matching entry here — re-derive it with the `config --format json`
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
demo:
	docker compose -f compose.dev.yaml --profile console down -v --remove-orphans
	for svc in sms-gateway migrate seed-demo-app sms-fake-orange sms-worker admin; do \
		COMPOSE_BAKE=false docker compose -f compose.dev.yaml --profile console build "$svc"; \
	done
	docker compose -f compose.dev.yaml --profile console up -d --wait

# Stop everything `just demo` started and remove its volumes (scratch
# Postgres data, provisioned secrets) — by exact Compose project name
# (`vsms-dev`) only, never touching `compose.yml`'s own `vsms` project or
# anything unrelated on the machine.
demo-down:
	docker compose -f compose.dev.yaml --profile console down -v --remove-orphans

# What's currently running from `just demo`.
demo-status:
	docker compose -f compose.dev.yaml --profile console ps

# The generated password `just demo` provisioned for `demo@vsms.local` —
# printed once, to `provision-user`'s own container log, never stored
# anywhere (see backends/apps/sms-gateway/src/main.rs's own `ProvisionUser` doc).
demo-login:
	docker compose -f compose.dev.yaml logs provision-user

# #160: the joined integration story — brings up `just demo`'s stack,
# provisions a SECOND client against the same App ("external integrator"),
# then runs `ci/e2e-integration` (a small Rust tool, not a bash script —
# see its own module doc for why it has to run *inside* the Compose
# network rather than as a host process) to send as that integrator over
# real HTTP and poll GET /messages/{id} AS THE CONSOLE's own credential —
# the same route frontends/packages/gateway/src/messages.ts's getMessageById calls —
# until that exact message id reaches `delivered`. Fails loudly (non-zero
# exit) if any link in the chain breaks. See docs/runbooks/e2e-integration.md
# for what this proves, what it fakes (Orange, via sms-fake-orange — #36's
# handset gate is unaffected), and why both clients share one App.
#
# `.e2e/` (gitignored) is where the integrator's own key/id land on the
# host — `docker compose run --rm` removes its container immediately, so
# there is no `docker compose cp` source for it the way `provision-client`'s
# own long-lived container (from `up`, still present) is for the console's
# credential two lines below.
e2e-integration: demo
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
