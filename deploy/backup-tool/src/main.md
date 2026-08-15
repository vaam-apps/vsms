`vsms-backup` — the Rust replacement for
`deploy/{backup,restore,restore-drill,backup-entrypoint}.sh`. #69's
own framing is still the design: "backups that have never been
restored are not backups" — this binary's `restore-drill` subcommand
is that proof, not just `backup`/`restore` themselves.

# Why `pg_dump`/`pg_restore`/`rclone` stay external processes

This is not the `app/sms-migrate` shape (a hand-rolled SQL runner
replacing `psql \i`) — `pg_dump`'s custom format and `pg_restore`'s
selective/parallel replay are a real, non-trivial binary protocol with
no reason to reimplement, and `rclone` is the one piece of this
mechanism that is deliberately provider-agnostic (S3, B2, GCS, Azure
Blob, MinIO, or a bare local path, all behind the same three calls —
see `docs/runbooks/backup-restore.md`'s own "the bucket is the
operator's choice" section). What this binary *does* replace: every
`psql -Atc "..."` ad-hoc query (now typed functions over a real
`postgres::Client`, `db.rs`), the manifest's own hand-built JSON
(`manifest.rs`, `serde`-typed), and — the part that used to need a
second daemon — Alpine's busybox `crond` plus a hand-written
`/etc/crontabs/root` line (`schedule.rs`, an in-process cron-expression
scheduler with its own graceful-shutdown handling).

# Why this is a separate Cargo workspace, not `app/sms-backup`

See `Cargo.toml`'s own header — same reasoning `examples/rust` and
`sdks/rust/vsms-sdk-rust` already establish for staying out of the
root workspace, applied here because this crate has nothing to do
with the schema/framework the root workspace's `include_server_schema!`
memory budget and MSRV are actually about, and depends on nothing from
`crates/`/`app/` at all.
