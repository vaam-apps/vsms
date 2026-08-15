#![doc = include_str!("worker_locks.md")]

use chrono::{DateTime, Utc};
// `cratestack::sqlx` the module, not individual items — matches
// `backends/apps/sms-migrate/src/main.rs` and `backends/crates/sms-worker/src/jobs/
// reap_outbox.rs`'s own convention: `cargo xtask no-raw-sqlx`'s pattern
// matches the literal substring `sqlx::query`, so the raw call stays visible at
// the call site rather than hidden behind a braced `use`.
use cratestack::CoolError;
use cratestack::sqlx;

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
