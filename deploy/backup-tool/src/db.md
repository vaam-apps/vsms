Every direct-Postgres query this tool needs — everything that used to
be a `psql -Atc "..."` call inline in a shell script is a typed
function here instead. `pg_dump`/`pg_restore` themselves are never
reimplemented (see `pgtools.rs`'s own module doc for why they stay
external processes); this module only replaces the handful of plain
`SELECT`/`INSERT`/`DROP` statements the old scripts shelled `psql` out
for.

Plain `postgres::Client`, not the `cratestack` delegates the main vsms
workspace uses everywhere else — R1 (`CONTRIBUTING.md`) governs
*application* data access inside that workspace; this is a standalone
operational tool in a separate Cargo workspace with no schema/`cratestack`
dependency at all, operating on the database from *outside* the
application (dumping and restoring the whole thing, destroying and
recreating `public` wholesale) rather than reading or writing rows
through it. `NoTls`: every `DATABASE_URL` this tool is ever pointed at
is a plain-`postgres://` compose-internal or loopback connection, the
same posture every other direct-Postgres tool in this repo already
takes (`crates/sms-worker/src/lease.rs`, `app/sms-migrate`).
