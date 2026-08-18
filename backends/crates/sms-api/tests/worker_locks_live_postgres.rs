//! `workerLocks` (#57) against a real, fully migrated Postgres — the actual
//! `ProcedureRegistry` trait method, and a real `RoleLease` held over a
//! genuinely separate connection, not a hand-constructed `pg_locks` row.
//! Same "call the trait method, not a private helper" discipline
//! `replay_webhook_attempt_live_postgres.rs`'s own module doc documents.
//!
//! This is also the live proof behind `worker_locks.rs`'s own module-doc
//! claim, checked once by hand with two `psql` sessions while designing
//! this feature and pinned here so it stays proven rather than becoming
//! folklore: a granted two-key advisory lock is exactly one row in
//! `pg_locks`, a losing `pg_try_advisory_lock` leaves no row at all, and
//! `application_name` (set from `RoleLease::try_acquire`'s own `worker_id`
//! parameter, `backends/crates/sms-worker/src/lease.rs`) is what lets this
//! procedure answer "which node," not just "is it held."
//!
//! ```bash
//! cargo test -p sms-api --test worker_locks_live_postgres -- --ignored
//! ```

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, CratestackError, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack, procedures::ProcedureRegistry, procedures::worker_locks};
use sms_api::{HashPepper, Procedures};
use sms_worker::Role;
use sms_worker::lease::RoleLease;

/// #102: this binary's own tests can race on Postgres's own `pg_type`
/// catalog the first time two of them prepare the exact same not-yet-cached
/// query shape at the same instant — see `backends/crates/sms-worker/tests/
/// claim_live_postgres.rs`'s own `TEST_MUTEX` doc for the full reasoning.
/// Load-bearing here for a second reason too: every test in this file
/// takes and releases real advisory locks under this deployment's fixed
/// `(dispatch, drain, scheduler, smpp)` keys, and two tests holding the
/// same role's lock at once would race lock *state*, not just query
/// preparation — the same reasoning `backends/crates/sms-worker/tests/live_postgres.rs`
/// already documents for its own four tests.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn app_caller_with_worker_read() -> CratestackContext {
    let mut ctx = Principal {
        sub: "worker-locks-test-console-client".to_owned(),
        kind: PrincipalKind::App,
        role: "app".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions.insert(
        "scope".to_owned(),
        Value::String("sms:send worker:read".to_owned()),
    );
    ctx
}

fn app_caller_without_worker_read() -> CratestackContext {
    let mut ctx = Principal {
        sub: "worker-locks-test-console-client-no-scope".to_owned(),
        kind: PrincipalKind::App,
        role: "app".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions
        .insert("scope".to_owned(), Value::String("sms:send".to_owned()));
    ctx
}

fn test_pepper() -> HashPepper {
    HashPepper::new("worker-locks-live-postgres-test-pepper-well-over-the-minimum-length")
        .expect("test pepper meets HashPepper::new's minimum length")
}

async fn db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

fn find_role<'a>(locks: &'a [schema::WorkerLockInfo], role: &str) -> &'a schema::WorkerLockInfo {
    locks
        .iter()
        .find(|l| l.role == role)
        .unwrap_or_else(|| panic!("workerLocks did not report role {role:?} at all"))
}

/// The headline case: a real `RoleLease` held under a known `worker_id`
/// shows up as `held: true`, with that exact `workerId` and a `pid`/
/// `heldSince` — proving the whole chain (`RoleLease::try_acquire`'s
/// `application_name`, `worker_locks.rs`'s `pg_locks` join, and the
/// procedure's own row-per-role assembly) works end to end, not just each
/// piece in isolation.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_held_lease_is_reported_with_its_workers_id() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let url = sms_test_support::database_url().await;

    let held = RoleLease::try_acquire(&url, Role::Dispatch, "test-node-alpha")
        .await
        .expect("attempting the dispatch lock")
        .expect("this test holds the only attempt at the dispatch lock this tick");

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_worker_read();
    let args = worker_locks::Args {};
    let snapshot = worker_locks::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.worker_locks(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("workerLocks must succeed for a caller with worker:read");

    let dispatch = find_role(&snapshot.locks, "dispatch");
    assert!(dispatch.singleton, "dispatch is a singleton role (§7.1)");
    assert!(dispatch.held, "the lease held above must be visible");
    assert_eq!(dispatch.workerId.as_deref(), Some("test-node-alpha"));
    assert!(
        dispatch.pid.is_some(),
        "a held lease must carry a real backend pid"
    );
    assert!(
        dispatch.heldSince.is_some(),
        "a held lease must carry when its connection started"
    );

    held.release().await.expect("releasing the test lease");
}

/// A role nothing has ever locked (in this test's own isolated moment) is
/// reported as `held: false` — not omitted, and not an error. Uses
/// `scheduler`, disjoint from the role the previous test locks, so the two
/// can never race even without the mutex (kept anyway, per this file's own
/// module doc, for the `pg_type` reason).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn an_unheld_singleton_role_is_reported_as_not_held() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // cratestack 0.7.13 (cratestack#512): see the identical comment on the
    // test above.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_worker_read();
    let args = worker_locks::Args {};
    let snapshot = worker_locks::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.worker_locks(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("workerLocks must succeed for a caller with worker:read");

    let scheduler = find_role(&snapshot.locks, "scheduler");
    assert!(
        !scheduler.held,
        "nothing in this test holds the scheduler lock"
    );
    assert!(scheduler.workerId.is_none());
    assert!(scheduler.pid.is_none());
    assert!(scheduler.heldSince.is_none());
}

/// `hooks`/`jobs` are `Cardinality::ScaleToN` — no lease exists for either
/// to hold, ever, in production. `workerLocks` still reports both by name
/// (§7.1's six roles, not four), each correctly flagged `singleton: false`
/// so the admin Workers screen can say "runs scale-to-N" instead of
/// implying a stuck lease.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn scale_to_n_roles_are_reported_as_non_singleton_and_unheld() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // cratestack 0.7.13 (cratestack#512): see the identical comment above.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_worker_read();
    let args = worker_locks::Args {};
    let snapshot = worker_locks::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.worker_locks(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("workerLocks must succeed for a caller with worker:read");

    assert_eq!(snapshot.locks.len(), 6, "all six §7.1 roles, always");
    for name in ["hooks", "jobs"] {
        let role = find_role(&snapshot.locks, name);
        assert!(!role.singleton, "{name} is Cardinality::ScaleToN");
        assert!(!role.held, "{name} never takes an advisory lock");
    }
}

/// Layer 2 (§5.1): an app-kind caller with no `worker:read` scope is
/// denied before the query ever runs.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn worker_locks_denies_a_caller_with_no_worker_read_scope() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — which runs the real `@allow` (Layer 1) first. This
    // caller's `auth().kind == "app"` still satisfies Layer 1 unconditionally
    // (`schema.cstack`'s `workerLocks` `@allow`), so this stays a genuine
    // Layer 2 (`require_permission`) denial, not a Layer 1 one.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_without_worker_read();
    let args = worker_locks::Args {};
    let error = worker_locks::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.worker_locks(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect_err("a caller with no worker:read scope must be denied");

    assert!(
        matches!(error, CratestackError::Forbidden(_)),
        "expected Forbidden, got {error:?}"
    );
    if let CratestackError::Forbidden(message) = error {
        assert!(
            message.contains("worker:read"),
            "expected the denial to name the missing permission: {message}"
        );
    }
}

/// Releasing a lease frees it immediately — the next `workerLocks` read
/// (not just the next `try_acquire`) must see `held: false` right away,
/// not eventually.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn releasing_a_lease_is_reflected_immediately() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let url = sms_test_support::database_url().await;

    let held = RoleLease::try_acquire(&url, Role::Smpp, "test-node-beta")
        .await
        .expect("attempting the smpp lock")
        .expect("this test holds the only attempt at the smpp lock this tick");

    // cratestack 0.7.13 (cratestack#512): see the identical comment above.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_worker_read();
    let args = worker_locks::Args {};
    let while_held = worker_locks::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.worker_locks(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("workerLocks must succeed");
    assert!(find_role(&while_held.locks, "smpp").held);

    held.release().await.expect("releasing the test lease");

    let after_release = worker_locks::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.worker_locks(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("workerLocks must succeed");
    assert!(
        !find_role(&after_release.locks, "smpp").held,
        "a released lease must stop being reported as held immediately"
    );
}
