The one-shot migration runner — §9.2's "which component applies
migrations, when, and what happens if two instances start at once."

Replaces `deploy/migrate.Dockerfile`'s old `postgres:16-alpine` + `psql`
shell script (`deploy/migrate.sql`) with a small Rust binary that embeds
the same SQL at *compile* time via [`include_str!`] rather than `COPY`ing
`.sql` files into the image and shelling out to `psql` at *run* time.
`cratestack-sqlx` already vendors `sqlx-core`/`sqlx-postgres` under
`cratestack::sqlx` (`backends/crates/sms-api`'s own `main.rs` already uses
`cratestack::sqlx::postgres::PgPoolOptions`) — this binary is the first
to reach past that facade for [`cratestack::sqlx::raw_sql`], the one
piece deploy/migrate.sql's own logic actually needs and `CrateStack`'s
own generated delegates have no reason to expose.

This is **not** an adoption of `sqlx`'s own migration framework (the
`sqlx::migrate!` macro, its `_sqlx_migrations` bookkeeping table, its
`<version>_<description>.sql` naming convention). That framework's own
directory scanner (`sqlx-core`'s `MigrationSource for &Path`, already
reachable through `cratestack::sqlx::migrate` with no new dependency)
only reads flat files directly inside one directory, named
`<VERSION>_<DESCRIPTION>.sql`, and silently skips anything that isn't a
file — confirmed by reading that resolver's own source, not assumed.
It cannot see this repo's `<name>/up.sql` layout at all, and that
layout isn't ours to change: `cratestack migrate diff --out-dir
backends/migrations/postgres --name <name>` is what produces it, and
`AGENTS.md`'s "never hand-edit" rule means the fix is not reshaping
generated output to fit a library's convention. Instead this ports
`deploy/migrate.sql`'s own `schema_migrations` bookkeeping logic
statement-for-statement into Rust: same advisory lock, same
per-migration "already applied?" check. The `.sql` this runs is
unchanged, byte-for-byte, from what `psql \i` used to run — only the
runner changed.

# Discovery, not a hand-maintained list

[`build.rs`](build.rs) walks `backends/migrations/postgres` at compile
time and generates the `MIGRATIONS` array below from whatever
subdirectories it finds an `up.sql` in, in lexical order — the same
directory `ci/apply-migrations.sh`'s own glob loop and
`backends/crates/sms-test-support`'s own `migration_dirs()` already read. A
fourth migration lands in `MIGRATIONS` the moment its directory exists;
nothing in this file needs editing. The library-bookkeeping DDL that
used to live at `ci/idempotency-table.sql` and be a hand-maintained
fourth entry has relocated to `backends/migrations/postgres/
0003_idempotency_table/up.sql` for exactly this reason — see that
file's own header for why it belongs in this directory at all despite
not being `cratestack`-generated.

# R1

Raw `sqlx` calls, not a `CrateStack` delegate — `cargo xtask no-raw-sqlx`
allowlists this file by path. This is not a new exception: "migrations"
was already one of R1's four named exceptions
(`CONTRIBUTING.md`) before this binary existed; `deploy/migrate.sql`
was simply the exception's previous form.

# Why a dedicated `PgConnection`, not a `Pool`

`pg_advisory_lock` is session-scoped: the lock lives on whichever
Postgres backend process holds the connection, and is released the
moment that session ends. `backends/crates/sms-worker/src/lease.rs` already
documents the trap a `Pool`'s `PoolConnection` sets here — it returns to
the pool rather than closing on drop, so a lock taken on a pooled
connection can silently outlive the code that took it, on a session a
later, unrelated query then reuses. A one-shot process like this one
exits immediately after either success or a propagated `Err`, so a bare
`PgConnection` closing on drop is sufficient — Postgres releases the
advisory lock the instant that socket closes, no explicit unlock
required on the error path. The happy path still unlocks explicitly
(matching `deploy/migrate.sql`'s own final statement), simply because
it costs nothing and keeps the two paths symmetric to read.
