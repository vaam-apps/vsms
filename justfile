# vsms — task runner.
#
# Expanding 16 models through `include_server_schema!` is memory-hungry enough
# to get rustc OOM-killed on a 32 GB machine at cargo's default job count. The
# recipes below cap concurrency rather than leaving each developer to discover
# that the hard way. See the `[profile.dev]` note in Cargo.toml.

# Cap build concurrency. Raise on a machine with headroom: `just jobs=8 check`.
jobs := "4"

_cargo := "CARGO_BUILD_JOBS=" + jobs + " cargo"

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

# Run every live-Postgres suite. `sms-test-support` starts (or reuses) one
# shared, self-healing Postgres 16 container and applies both migrations —
# needs Docker, nothing else. Safe to rerun: the container is named
# deterministically and reused, not recreated, across runs.
test-live:
	{{_cargo}} test --workspace -- --ignored

# Remove the shared test-harness container. Scoped by the exact label
# `sms-test-support` itself sets (`dev.vsms.test-harness=true`) — never by
# image or a bare name pattern, so this can never touch a container this
# workspace's tests did not create. Not required for correctness (the
# harness self-heals on its own next run) — this is just for a developer
# who wants their machine back to zero right now.
test-live-clean:
	docker rm -f $(docker ps -aq --filter "label=dev.vsms.test-harness=true") 2>/dev/null || true

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
	./ci/assert-no-raw-sqlx.sh
	python3 ci/assert-state-machine-parity.py

# R2: the state diagram and the transition table must agree
parity:
	python3 ci/assert-state-machine-parity.py

# Print the generated route table. Needs no database.
routes:
	{{_cargo}} run -p sms-gateway -- routes

# Apply migrations to a scratch database, run the state-machine assertions, drop it
schema-check:
	createdb vsms_check
	DATABASE_URL=postgres://localhost/vsms_check ./ci/apply-migrations.sh
	psql postgres://localhost/vsms_check -v ON_ERROR_STOP=1 -f ci/test-state-machine.sql
	dropdb vsms_check

# Regenerate 0002_bootstrap from §2.10 of the design doc
bootstrap-sql:
	python3 ci/gen-bootstrap-sql.py schema/migrations/postgres/0002_bootstrap/up.sql
