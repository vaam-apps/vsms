//! Leader election by Postgres advisory lock. §7.2 of the design doc.
//!
//! **R1 exception** — one of the named ones (migrations, `pg_advisory_lock`,
//! `LISTEN`/`NOTIFY`). `cargo xtask no-raw-sqlx` allowlists this exact file
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

use std::str::FromStr;
use std::time::Duration;

use cratestack::sqlx::postgres::{PgConnectOptions, PgConnection};
use cratestack::sqlx::{query_scalar, Connection};
use sms_api::worker_roles::{lock_key_for_role, ADVISORY_LOCK_NAMESPACE};

use crate::Role;

/// The advisory-lock "class" every role's lock lives under. `pg_advisory_lock`
/// takes a `(classid, objid)` pair; every call in this crate uses this same
/// class, so `objid` alone (via [`advisory_lock_key`]) has to be unique per
/// role — verified by `advisory_lock_keys_are_all_distinct` below.
///
/// Re-exported from `sms_api::worker_roles` rather than defined here — see
/// that module's own doc for why: `Procedures::worker_locks` (#57) reads
/// `pg_locks` back and has to agree with this crate's own key byte-for-byte,
/// and `sms-api` cannot depend back on `sms-worker` (`sms-worker` already
/// depends on `sms-api`), so the shared fact lives on the side of that
/// one-directional edge, not duplicated on both.
const NS: i32 = ADVISORY_LOCK_NAMESPACE;

/// The `(classid, objid)` pair for a role. Looks up
/// [`sms_api::worker_roles::ROLE_LOCK_KEYS`] by [`Role::as_str`] rather than
/// matching on `Role` directly — the `.expect` is the same "must classify
/// every role" discipline the old exhaustive match gave for free, just
/// re-anchored on the shared table instead of a local one; `role_lock_keys_
/// are_all_distinct` (`sms-api`'s own test) and `advisory_lock_keys_are_all_
/// distinct` below both still guard uniqueness, one per side of the
/// dependency edge.
fn advisory_lock_key(role: Role) -> i32 {
    lock_key_for_role(role.as_str())
        .unwrap_or_else(|| panic!("no advisory lock key registered for role {role}"))
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
    /// `worker_id` is stamped onto the connection's own `application_name`
    /// (#57) — the one thing that lets `Procedures::worker_locks`' `pg_locks`
    /// query answer "which node" rather than just "is it held": the winning
    /// connection is dedicated to nothing but holding this one lock (this
    /// module's own doc explains why), so its `pg_stat_activity.
    /// application_name` is, for as long as the lease lives, this worker's
    /// own identity — the same `--worker-id`/`SMS_WORKER_ID` value
    /// `app/sms-worker/src/main.rs` already threads through every claim
    /// loop's `leaseOwner`. A losing connection carries the same
    /// `application_name` too, briefly, but it closes itself a few lines
    /// below before that's ever observable from outside this function.
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
    pub async fn try_acquire(
        database_url: &str,
        role: Role,
        worker_id: &str,
    ) -> Result<Option<Self>, LeaseError> {
        let options = PgConnectOptions::from_str(database_url)
            .map_err(|source| LeaseError::Connect { role, source })?
            .application_name(worker_id);
        let mut conn = PgConnection::connect_with(&options)
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
    ///
    /// `pg_advisory_unlock` itself returns a boolean — `true` if this
    /// session actually held the lock, `false` otherwise — which a plain
    /// `execute()` would discard, leaving `Ok(())` from this function claim
    /// more than Postgres actually confirmed. Every `RoleLease` in existence
    /// came from a `try_acquire` that confirmed the lock was held, so `false`
    /// should be unreachable in practice; checked anyway, because "unreachable
    /// in practice" is exactly the assumption a future refactor could break
    /// silently otherwise.
    #[must_use = "an Err here means the explicit unlock may not have reached Postgres"]
    pub async fn release(mut self) -> Result<(), LeaseError> {
        let released: bool = query_scalar("SELECT pg_advisory_unlock($1, $2)")
            .bind(NS)
            .bind(advisory_lock_key(self.role))
            .fetch_one(&mut self.conn)
            .await
            .map_err(|source| LeaseError::Query {
                role: self.role,
                source,
            })?;

        if !released {
            tracing::warn!(
                role = %self.role,
                "pg_advisory_unlock reported this session did not hold the lock; \
                 a RoleLease should never reach release() in that state"
            );
        }

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

    /// The executable half of `sms_api::worker_roles`'s own doc promise: its
    /// `is_singleton` table must agree with `Role::cardinality`, the type
    /// this crate actually dispatches on, or #57's Workers screen would
    /// silently mislabel a role as "scale-to-N, no lock expected" (or vice
    /// versa) the moment the two drifted. This crate can depend on
    /// `sms-api` (the reverse can't — see that module's own doc), so this is
    /// the one side of the edge that can actually write the check.
    #[test]
    fn sms_apis_singleton_table_agrees_with_role_cardinality() {
        for role in ALL {
            let expected = matches!(role.cardinality(), crate::Cardinality::Singleton);
            let actual = sms_api::worker_roles::is_singleton(role.as_str())
                .unwrap_or_else(|| panic!("sms_api::worker_roles has no entry for {role}"));
            assert_eq!(
                actual, expected,
                "{role}: sms_api::worker_roles says singleton={actual}, \
                 Role::cardinality says singleton={expected}"
            );
        }
    }
}
