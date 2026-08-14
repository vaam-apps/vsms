//! The six §7.1 role names and the `pg_advisory_lock` key each one takes
//! when it runs as a singleton — the single source of truth shared between
//! the process that *takes* a lock (`backends/crates/sms-worker/src/lease.rs`) and
//! the process that *reads it back* (`Procedures::worker_locks`, #57, via
//! `worker_locks.rs`'s `pg_locks` query).
//!
//! This lives in `sms-api`, not `sms-worker`, on purpose: `sms-worker`
//! already depends on `sms-api` (for the expanded schema `claim.rs` claims
//! against — see that crate's own module doc), so `sms-api` cannot depend
//! back on `sms-worker` without a cycle. `sms-worker::Role` stays the real,
//! full type (cardinality, CLI parsing, `--roles` validation); this module
//! only mirrors the one fact both sides need to agree on byte-for-byte — the
//! `(name, objid)` pair — so lease-taking and lock-reading can never quietly
//! drift apart the way this repo's own `hasRole('system')` gap kept
//! recurring before it got a golden test (`AGENTS.md`'s "Invariants" section).
//!
//! `backends/crates/sms-worker/src/lease.rs::advisory_lock_key` sources its values
//! from [`lock_key_for_role`] rather than keeping its own copy, and
//! `backends/crates/sms-worker/src/lease.rs`'s own test module cross-checks every
//! [`Role`](../../sms-worker/src/lib.rs)'s `cardinality()` against
//! [`is_singleton`] here — see that test for the executable half of this
//! guarantee.

/// The advisory-lock "class" every role's lock lives under — the four ASCII
/// bytes `"SMS\0"`, per §7.2. `pg_advisory_lock` takes a `(classid, objid)`
/// pair; every role in this table shares this one `classid`, so `objid`
/// alone has to be unique per role (`role_lock_keys_are_all_distinct` below).
pub const ADVISORY_LOCK_NAMESPACE: i32 = 0x534d_5300;

/// `(role name, objid, singleton)` for all six §7.1 roles, in the same
/// order `sms_worker::ALL` lists them. `singleton` mirrors
/// `sms_worker::Role::cardinality`'s own `Singleton` arm (`Dispatch`,
/// `Drain`, `Scheduler`, `Smpp`) — `Hooks`/`Jobs` are `ScaleToN` and never
/// call `pg_try_advisory_lock` at all, so they can never appear as a row in
/// `pg_locks`; `Procedures::worker_locks` still reports them (`held: false`)
/// so the Workers screen can say plainly "this role runs scale-to-N, a held
/// lock isn't the question" rather than silently omitting two of the six
/// names §7.1 documents.
pub const ROLE_LOCK_KEYS: &[(&str, i32, bool)] = &[
    ("dispatch", 1, true),
    ("drain", 2, true),
    ("scheduler", 3, true),
    ("hooks", 4, false),
    ("jobs", 5, false),
    ("smpp", 6, true),
];

/// The `objid` a role's advisory lock is taken under, or `None` for a name
/// outside the six §7.1 roles. `backends/crates/sms-worker/src/lease.rs`'s
/// `advisory_lock_key` calls this rather than matching on `Role` itself —
/// see this module's own doc for why the lookup lives here.
#[must_use]
pub fn lock_key_for_role(name: &str) -> Option<i32> {
    ROLE_LOCK_KEYS
        .iter()
        .find(|(role, _, _)| *role == name)
        .map(|(_, objid, _)| *objid)
}

/// The role name an `objid` was taken under, or `None` for a value this
/// table doesn't recognise (a lock this codebase didn't take — reported by
/// `worker_locks.rs` as-is rather than silently dropped; see that module).
#[must_use]
pub fn role_for_lock_key(objid: i32) -> Option<&'static str> {
    ROLE_LOCK_KEYS
        .iter()
        .find(|(_, key, _)| *key == objid)
        .map(|(role, _, _)| *role)
}

/// Whether `name` is one of the four singleton roles — `Procedures::worker_locks`
/// echoes this back per row so the client never has to hardcode its own
/// copy of the singleton/scale-to-N split.
#[must_use]
pub fn is_singleton(name: &str) -> Option<bool> {
    ROLE_LOCK_KEYS
        .iter()
        .find(|(role, _, _)| *role == name)
        .map(|(_, _, singleton)| *singleton)
}

#[cfg(test)]
mod tests {
    use super::{ADVISORY_LOCK_NAMESPACE, ROLE_LOCK_KEYS};

    #[test]
    fn role_lock_keys_are_all_distinct() {
        let mut seen: Vec<i32> = Vec::new();
        for (role, key, _) in ROLE_LOCK_KEYS {
            assert!(!seen.contains(key), "{role} reuses advisory lock key {key}");
            seen.push(*key);
        }
    }

    /// Pinned for the same reason `sms_worker::lease`'s own copy of this
    /// assertion is: an edit here silently changes which lock a running
    /// deployment's `pg_locks` rows correspond to.
    #[test]
    fn the_namespace_is_the_designs_documented_constant() {
        assert_eq!(ADVISORY_LOCK_NAMESPACE, 0x534d_5300);
        assert_eq!(ADVISORY_LOCK_NAMESPACE.to_be_bytes(), *b"SMS\0");
    }

    #[test]
    fn six_roles_match_7_1s_table() {
        let names: Vec<&str> = ROLE_LOCK_KEYS.iter().map(|(role, _, _)| *role).collect();
        assert_eq!(
            names,
            vec!["dispatch", "drain", "scheduler", "hooks", "jobs", "smpp"]
        );
    }
}
