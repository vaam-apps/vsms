//! #57: which node holds which singleton-role advisory lock — `pg_locks`
//! joined against `pg_stat_activity`, read straight over the pool rather
//! than through a delegate.
//!
//! # A new, seventh R1 exception
//!
//! `pg_locks` is Postgres's own lock catalog, not one of `schema.cstack`'s
//! models — there is no table for a delegate to point at, so no delegate
//! exists to read it through. Same reasoning `crates/sms-worker/src/
//! drain.rs`'s `oldest_undelivered_age` and `crates/sms-worker/src/jobs/
//! reap_outbox.rs` already established for `cratestack_event_outbox` (a
//! different framework-internal, non-model table): nothing here bypasses
//! row-level policy (there is no row-level policy on a system catalog),
//! skips an audit trail (a `SELECT` writes no audit row regardless), or
//! sidesteps `@version`/soft-delete (neither concept applies to a catalog
//! view). `cargo xtask no-raw-sqlx` and `CONTRIBUTING.md`'s exceptions
//! table both name this file.
//!
//! # What `pg_locks` actually reports for a session advisory lock —
//! verified live against a real Postgres 16, not assumed from documentation
//!
//! `crates/sms-worker/src/lease.rs::RoleLease` takes its lock with
//! `pg_try_advisory_lock($1, $2)` — the two-argument, session-level,
//! non-blocking form. Two things checked directly, with two real `psql`
//! sessions and a third querying `pg_locks`, before writing the query below:
//!
//! - A granted two-key advisory lock is exactly one row: `locktype =
//!   'advisory'`, `classid = <namespace>`, `objid = <role key>`,
//!   `objsubid = 2` (the two-int form's own tag; the single-bigint form
//!   uses `1` instead — not used anywhere in this codebase, but worth
//!   naming so a future reader doesn't wonder), `granted = true`.
//! - **A `pg_try_advisory_lock` call that loses the race creates no row at
//!   all.** Unlike the blocking `pg_advisory_lock`, there is nothing to
//!   queue — the call returns `false` immediately, and the losing session's
//!   connection is closed by `RoleLease::try_acquire` itself a moment
//!   later. Confirmed live: with one session holding the lock, a second
//!   session's `pg_try_advisory_lock` on the identical `(classid, objid)`
//!   returned `f`, and a third, independent connection's `SELECT * FROM
//!   pg_locks WHERE locktype = 'advisory'` still showed exactly the one row
//!   — the winner's, unchanged.
//!
//! **The consequence for #57's own framing** ("two `dispatch` workers
//! means a blocked Orange account"): Postgres cannot show two granted rows
//! for the same `(classid, objid)` pair — a two-key advisory lock is
//! exclusive by construction, the identical guarantee that makes
//! `RoleLease` safe leader election in the first place. If two processes
//! were ever both genuinely acting as `dispatch` at once, `pg_locks` could
//! never surface that as two granted rows for the `dispatch` key — that
//! would mean a bug in code that bypasses `run_singleton`'s own
//! `try_acquire` gate entirely (or a future refactor that stops taking the
//! lock at all), not something this table could ever show directly. What
//! this screen *can* and does show, and what actually answers "is dispatch
//! running, and where": whether the role's lock is currently held at all,
//! by which node (`application_name`, set to the worker's own `--worker-id`
//! by [`crate::worker_roles`]'s caller in `lease.rs`), and since when (the
//! dedicated lease connection's own `backend_start` — that connection
//! exists for nothing but holding this one lock, so its session start time
//! is, to the second, when this attempt acquired it).

use chrono::{DateTime, Utc};
// `cratestack::sqlx` the module, not individual items — matches
// `app/sms-migrate/src/main.rs` and `crates/sms-worker/src/jobs/
// reap_outbox.rs`'s own convention: `cargo xtask no-raw-sqlx`'s pattern
// matches the literal substring `sqlx::query`, so the raw call stays visible at
// the call site rather than hidden behind a braced `use`.
use cratestack::sqlx;
use cratestack::CoolError;

use crate::schema::{Cratestack, WorkerLockInfo};
use crate::worker_roles::{ADVISORY_LOCK_NAMESPACE, ROLE_LOCK_KEYS};

/// `(objid, pid, application_name, backend_start)` for every currently
/// *granted* two-key advisory lock under this deployment's namespace. A
/// plain tuple, not a named struct — `sqlx`'s blanket `FromRow` impl for
/// tuples up to arity 9 covers this without needing a `#[derive(FromRow)]`
/// type that would exist for exactly one query.
type LockRow = (i32, i32, String, Option<DateTime<Utc>>);

/// One row per entry in [`ROLE_LOCK_KEYS`] — always all six, `held: false`
/// for whichever roles have no matching row in `pg_locks` right now. Never
/// partial: a role this deployment has never started shows up identically
/// to one whose lease briefly lapsed between two polls, which is the
/// correct, honest answer — "not held right now" is all either case can
/// truthfully claim.
///
/// # Errors
///
/// [`CoolError::Internal`] if the query itself fails — `pg_locks`/
/// `pg_stat_activity` are always-present system views, so unlike
/// `cratestack_event_outbox` there is no "table doesn't exist yet" case to
/// treat as empty.
pub async fn current_locks(db: &Cratestack) -> Result<Vec<WorkerLockInfo>, CoolError> {
    let rows: Vec<LockRow> = sqlx::query_as(
        "SELECT l.objid::int4 AS objid, \
                l.pid::int4 AS pid, \
                a.application_name AS application_name, \
                a.backend_start AS backend_start \
         FROM pg_locks l \
         JOIN pg_stat_activity a ON a.pid = l.pid \
         WHERE l.locktype = 'advisory' AND l.classid::int4 = $1 AND l.granted",
    )
    .bind(ADVISORY_LOCK_NAMESPACE)
    .fetch_all(db.pool())
    .await
    .map_err(|error| CoolError::Internal(format!("reading pg_locks for worker leases: {error}")))?;

    Ok(ROLE_LOCK_KEYS
        .iter()
        .map(|(name, objid, singleton)| {
            let holder = rows.iter().find(|(row_objid, ..)| row_objid == objid);
            WorkerLockInfo {
                role: (*name).to_owned(),
                singleton: *singleton,
                held: holder.is_some(),
                workerId: holder.map(|(_, _, application_name, _)| application_name.clone()),
                pid: holder.map(|(_, pid, ..)| i64::from(*pid)),
                heldSince: holder.and_then(|(_, _, _, backend_start)| *backend_start),
            }
        })
        .collect())
}
