//! Leader election by Postgres advisory lock. §7.2 of the design doc.
//!
//! **R1 exception** — one of three named ones (migrations, `pg_advisory_lock`,
//! `LISTEN`/`NOTIFY`). `ci/assert-no-raw-sqlx.sh` allowlists this exact file
//! by path; adding a raw `sqlx::query*` call anywhere else in this crate is
//! still a build failure, on purpose.
//!
//! # The trap this module is built to avoid
//!
//! §7.2's own illustrative code sample acquires the lock connection with
//! `pool.acquire()` from a shared, multi-connection `PgPool`. That is exactly
//! the shape the doc's own prose warns against: `sqlx`'s `PoolConnection`
//! returns itself to the pool *on drop* rather than closing the socket, so a
//! dropped lease-holding connection doesn't release anything at the Postgres
//! level — the session stays open, recycled to serve some unrelated future
//! query, and the lock it silently still holds is now unreachable. No other
//! node can ever take that role until the whole process restarts, and
//! nothing about that failure is loud.
//!
//! [`RoleLease`] sidesteps the trap by never using a shared pool at all: it
//! owns a single, dedicated [`PgConnection`] that belongs to nothing else.
//! Dropping it (a panic, a `kill -9`) closes the socket immediately at the
//! OS level; Postgres's own session-advisory-lock semantics release the lock
//! the moment that session ends, with zero cooperation required from this
//! crate or from `sqlx`. [`RoleLease::release`] is still the fast path —
//! signal-then-drop can lag behind an explicit unlock by however long TCP
//! failure detection takes — but it is a latency optimisation on top of a
//! mechanism that is already correct without it, not the only thing
//! standing between a clean release and a leak.

use std::time::Duration;

use cratestack::sqlx::postgres::PgConnection;
use cratestack::sqlx::{query, query_scalar, Connection};

use crate::Role;

/// The advisory-lock "class" every role's lock lives under. `pg_advisory_lock`
/// takes a `(classid, objid)` pair; every call in this crate uses this same
/// class, so `objid` alone (via [`advisory_lock_key`]) has to be unique per
/// role — verified by `advisory_lock_keys_are_all_distinct` below.
const NS: i32 = 0x534d_5300; // "SMS\0"

/// The `(classid, objid)` pair for a role, distinct per role by construction:
/// an exhaustive match with one arm per [`Role`] variant, so the compiler
/// refuses to compile a new role that forgot to claim a key here.
const fn advisory_lock_key(role: Role) -> i32 {
    match role {
        Role::Dispatch => 1,
        Role::Drain => 2,
        Role::Scheduler => 3,
        Role::Hooks => 4,
        Role::Jobs => 5,
        Role::Smpp => 6,
    }
}

/// A held Postgres session-level advisory lock for one [`Role`].
///
/// Holding one of these *is* holding the singleton role — there is
/// deliberately no other state anywhere claiming to track that. Drop it (or
/// call [`RoleLease::release`]) and the role is up for grabs again.
pub struct RoleLease {
    conn: PgConnection,
    role: Role,
}

/// Something went wrong talking to Postgres about a lease. Never means "the
/// lock is held elsewhere" — that's `Ok(None)` from [`RoleLease::try_acquire`],
/// a routine, expected outcome in any deployment running more than one node
/// per role. This type is only the failure case: connection refused, auth
/// failed, the query itself errored.
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// Couldn't even open the dedicated connection.
    #[error("connecting to attempt the {role} lock: {source}")]
    Connect {
        /// Which role's lock this connection attempt was for.
        role: Role,
        /// The underlying `sqlx` connection failure.
        #[source]
        source: cratestack::sqlx::Error,
    },
    /// Connected, but the lock or unlock query itself failed.
    #[error("querying the {role} lock: {source}")]
    Query {
        /// Which role's lock this query was for.
        role: Role,
        /// The underlying `sqlx` query failure.
        #[source]
        source: cratestack::sqlx::Error,
    },
}

impl RoleLease {
    /// Attempt to acquire `role`'s lock over a fresh, dedicated connection.
    ///
    /// `Ok(None)` — someone else holds it. Routine; log it quietly and retry
    /// later, per §7.2: "a worker holding no lock for a singleton role isn't
    /// idle — it runs its scalable roles normally and retries the lock in
    /// the background."
    ///
    /// `Err(_)` — something is actually broken (unreachable database, bad
    /// credentials, ...). This is the case worth alerting on loudly: if every
    /// node hits this for the same role, the role goes unheld cluster-wide,
    /// which is the dangerous state §28 names explicitly.
    pub async fn try_acquire(database_url: &str, role: Role) -> Result<Option<Self>, LeaseError> {
        let mut conn = PgConnection::connect(database_url)
            .await
            .map_err(|source| LeaseError::Connect { role, source })?;

        let acquired: bool = query_scalar("SELECT pg_try_advisory_lock($1, $2)")
            .bind(NS)
            .bind(advisory_lock_key(role))
            .fetch_one(&mut conn)
            .await
            .map_err(|source| LeaseError::Query { role, source })?;

        if acquired {
            Ok(Some(Self { conn, role }))
        } else {
            // Not the winner — this connection has nothing left to do.
            // Closing it explicitly (rather than dropping and letting the
            // socket close implicitly) costs one round trip and avoids
            // leaving a half-torn-down connection for the driver to clean
            // up asynchronously while we're about to open a fresh one in
            // 5 seconds anyway.
            let _ = conn.close().await;
            Ok(None)
        }
    }

    /// Release the lock and close the connection, in that order, so the
    /// unlock is what a concurrent `pg_try_advisory_lock` from another node
    /// actually observes rather than a connection simply vanishing.
    ///
    /// Not required for correctness — see this module's doc for why an
    /// abandoned [`RoleLease`] still releases the lock on its own — but it's
    /// the fast path, and the one place in this crate `#[must_use]` on the
    /// return matters: an ignored error here means the explicit release may
    /// not have happened, silently falling back to the slower drop-triggered
    /// path.
    #[must_use = "an Err here means the explicit unlock may not have reached Postgres"]
    pub async fn release(mut self) -> Result<(), LeaseError> {
        query("SELECT pg_advisory_unlock($1, $2)")
            .bind(NS)
            .bind(advisory_lock_key(self.role))
            .execute(&mut self.conn)
            .await
            .map_err(|source| LeaseError::Query {
                role: self.role,
                source,
            })?;

        let _ = self.conn.close().await;
        Ok(())
    }
}

/// How long to wait between advisory-lock attempts. §7.2: "a standby node
/// acquires it on its next attempt (retry every 5 seconds)."
pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::{advisory_lock_key, NS};
    use crate::ALL;

    #[test]
    fn advisory_lock_keys_are_all_distinct() {
        let mut seen: Vec<i32> = Vec::new();
        for role in ALL {
            let key = advisory_lock_key(role);
            assert!(
                !seen.contains(&key),
                "{role} reuses advisory lock key {key}, already claimed by another role"
            );
            seen.push(key);
        }
    }

    /// Pinned so an edit doesn't silently change which lock a running
    /// deployment's rows in `pg_locks` correspond to — the namespace is
    /// exactly the four ASCII bytes "SMS\0", per §7.2.
    #[test]
    fn the_namespace_is_the_designs_documented_constant() {
        assert_eq!(NS, 0x534d_5300);
        assert_eq!(NS.to_be_bytes(), *b"SMS\0");
    }
}
