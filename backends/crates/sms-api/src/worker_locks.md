#57: which node holds which singleton-role advisory lock — `pg_locks`
joined against `pg_stat_activity`, read straight over the pool rather
than through a delegate.

# A new, seventh R1 exception

`pg_locks` is Postgres's own lock catalog, not one of `schema.cstack`'s
models — there is no table for a delegate to point at, so no delegate
exists to read it through. Same reasoning `backends/crates/sms-worker/src/
drain.rs`'s `oldest_undelivered_age` and `backends/crates/sms-worker/src/jobs/
reap_outbox.rs` already established for `cratestack_event_outbox` (a
different framework-internal, non-model table): nothing here bypasses
row-level policy (there is no row-level policy on a system catalog),
skips an audit trail (a `SELECT` writes no audit row regardless), or
sidesteps `@version`/soft-delete (neither concept applies to a catalog
view). `cargo xtask no-raw-sqlx` and `CONTRIBUTING.md`'s exceptions
table both name this file.

# What `pg_locks` actually reports for a session advisory lock —
verified live against a real Postgres 16, not assumed from documentation

`backends/crates/sms-worker/src/lease.rs::RoleLease` takes its lock with
`pg_try_advisory_lock($1, $2)` — the two-argument, session-level,
non-blocking form. Two things checked directly, with two real `psql`
sessions and a third querying `pg_locks`, before writing the query below:

- A granted two-key advisory lock is exactly one row: `locktype =
  'advisory'`, `classid = <namespace>`, `objid = <role key>`,
  `objsubid = 2` (the two-int form's own tag; the single-bigint form
  uses `1` instead — not used anywhere in this codebase, but worth
  naming so a future reader doesn't wonder), `granted = true`.
- **A `pg_try_advisory_lock` call that loses the race creates no row at
  all.** Unlike the blocking `pg_advisory_lock`, there is nothing to
  queue — the call returns `false` immediately, and the losing session's
  connection is closed by `RoleLease::try_acquire` itself a moment
  later. Confirmed live: with one session holding the lock, a second
  session's `pg_try_advisory_lock` on the identical `(classid, objid)`
  returned `f`, and a third, independent connection's `SELECT * FROM
  pg_locks WHERE locktype = 'advisory'` still showed exactly the one row
  — the winner's, unchanged.

**The consequence for #57's own framing** ("two `dispatch` workers
means a blocked Orange account"): Postgres cannot show two granted rows
for the same `(classid, objid)` pair — a two-key advisory lock is
exclusive by construction, the identical guarantee that makes
`RoleLease` safe leader election in the first place. If two processes
were ever both genuinely acting as `dispatch` at once, `pg_locks` could
never surface that as two granted rows for the `dispatch` key — that
would mean a bug in code that bypasses `run_singleton`'s own
`try_acquire` gate entirely (or a future refactor that stops taking the
lock at all), not something this table could ever show directly. What
this screen *can* and does show, and what actually answers "is dispatch
running, and where": whether the role's lock is currently held at all,
by which node (`application_name`, set to the worker's own `--worker-id`
by [`crate::worker_roles`]'s caller in `lease.rs`), and since when (the
dedicated lease connection's own `backend_start` — that connection
exists for nothing but holding this one lock, so its session start time
is, to the second, when this attempt acquired it).
