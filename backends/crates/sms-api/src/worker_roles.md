The six §7.1 role names and the `pg_advisory_lock` key each one takes
when it runs as a singleton — the single source of truth shared between
the process that *takes* a lock (`backends/crates/sms-worker/src/lease.rs`) and
the process that *reads it back* (`Procedures::worker_locks`, #57, via
`worker_locks.rs`'s `pg_locks` query).

This lives in `sms-api`, not `sms-worker`, on purpose: `sms-worker`
already depends on `sms-api` (for the expanded schema `claim.rs` claims
against — see that crate's own module doc), so `sms-api` cannot depend
back on `sms-worker` without a cycle. `sms-worker::Role` stays the real,
full type (cardinality, CLI parsing, `--roles` validation); this module
only mirrors the one fact both sides need to agree on byte-for-byte — the
`(name, objid)` pair — so lease-taking and lock-reading can never quietly
drift apart the way this repo's own `hasRole('system')` gap kept
recurring before it got a golden test (`AGENTS.md`'s "Invariants" section).

`backends/crates/sms-worker/src/lease.rs::advisory_lock_key` sources its values
from [`lock_key_for_role`] rather than keeping its own copy, and
`backends/crates/sms-worker/src/lease.rs`'s own test module cross-checks every
[`Role`](../../sms-worker/src/lib.rs)'s `cardinality()` against
[`is_singleton`] here — see that test for the executable half of this
guarantee.
