//! Proves [`RoleLease`] against a real Postgres — in particular, that
//! dropping a lease without calling `release()` still frees the lock. That's
//! the entire reason this module exists instead of the §7.2 sample code's
//! `pool.acquire()` shape; a unit test can't demonstrate it since it needs a
//! second, independent connection to observe the lock's state from outside.
//!
//! Ignored by default, same convention as `sms-auth`'s live suite. `sms_
//! test_support` provisions Postgres automatically (a shared, self-healing
//! container — see its own module doc), so running this needs only Docker
//! and:
//!
//! ```bash
//! cargo test -p sms-worker --test live_postgres -- --ignored
//! ```
//!
//! No migration is strictly needed for what this file tests — advisory
//! locks aren't backed by any table this schema declares, an empty
//! freshly-created database is enough — but `sms_test_support::database_url`
//! applies them anyway, unconditionally, so every live suite shares the one
//! container regardless of what any individual suite happens to need.

use sms_worker::lease::RoleLease;
use sms_worker::{Role, WorkerContext};
use std::time::Duration;

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests — run concurrently by Rust's default multi-threaded test
/// harness — can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape at the
/// same instant. See `crates/sms-worker/tests/claim_live_postgres.rs`'s
/// own `TEST_MUTEX` doc for the full reasoning — same mechanism, same
/// fix. This file predates `ca653a1` (#102) — its four tests were never
/// updated to pick up the pattern, and being `#[ignore]`d by convention
/// meant nothing caught the gap until #118 started running every live
/// suite automatically in CI. The four tests using distinct `Role`
/// values (see `different_roles_do_not_contend_with_each_other`'s own
/// comment) avoids racing on lock *state*, but not on the unrelated
/// `pg_type` catalog race, which is about first-time query-shape
/// preparation, not about which advisory-lock key is held.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

#[tokio::test]
#[ignore = "needs a live Postgres — see module docs"]
async fn a_second_attempt_for_the_same_role_is_none_while_the_first_holds_it() {
    let _guard = TEST_MUTEX.lock().await;
    let url = sms_test_support::database_url().await;

    let first = RoleLease::try_acquire(&url, Role::Dispatch, "worker-a")
        .await
        .expect("first attempt succeeds")
        .expect("first attempt is the winner");

    let second = RoleLease::try_acquire(&url, Role::Dispatch, "worker-b")
        .await
        .expect("second attempt doesn't error just because it lost");
    assert!(
        second.is_none(),
        "a second holder would mean two dispatchers"
    );

    first.release().await.expect("releasing the first lease");
}

#[tokio::test]
#[ignore = "needs a live Postgres — see module docs"]
async fn releasing_frees_the_lock_for_the_next_attempt() {
    let _guard = TEST_MUTEX.lock().await;
    let url = sms_test_support::database_url().await;

    let first = RoleLease::try_acquire(&url, Role::Drain, "worker-a")
        .await
        .unwrap()
        .expect("first attempt is the winner");
    first.release().await.expect("releasing");

    let second = RoleLease::try_acquire(&url, Role::Drain, "worker-b")
        .await
        .unwrap();
    assert!(
        second.is_some(),
        "release() must free the lock for the very next attempt, not eventually"
    );
}

/// The reason this module exists. If `RoleLease` used `pool.acquire()` from
/// a shared pool instead of a dedicated connection, this test would fail:
/// the dropped `PoolConnection` would return to the pool rather than close,
/// leaving the lock held by a session nothing can reach anymore.
#[tokio::test]
#[ignore = "needs a live Postgres — see module docs"]
async fn dropping_an_unreleased_lease_still_frees_the_lock() {
    let _guard = TEST_MUTEX.lock().await;
    let url = sms_test_support::database_url().await;

    let first = RoleLease::try_acquire(&url, Role::Scheduler, "worker-a")
        .await
        .unwrap()
        .expect("first attempt is the winner");

    drop(first); // no .release() — simulating a panic or a `kill -9`

    // Dropping closes the socket, but the close is not instant from this
    // task's point of view (no `.await` happens in `Drop`) — give Postgres a
    // moment to notice the session ended before asserting on it. Generous on
    // purpose: this is proving a property, not measuring latency.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let second = RoleLease::try_acquire(&url, Role::Scheduler, "worker-b")
        .await
        .unwrap();
    assert!(
        second.is_some(),
        "a lease dropped without release() must not leak the lock forever"
    );
}

/// `Hooks`/`Jobs` here purely as two role labels distinct from what the
/// other three tests in this file use — `cargo test` runs test functions in
/// this binary concurrently by default, and advisory locks are real cluster
/// state, so any two tests sharing a role would be a race on the lock
/// itself, not just on assertions. Nothing about this test cares that
/// `Hooks`/`Jobs` are `ScaleToN` in production; `try_acquire` doesn't check
/// cardinality, so any two distinct roles prove the same thing.
#[tokio::test]
#[ignore = "needs a live Postgres — see module docs"]
async fn different_roles_do_not_contend_with_each_other() {
    let _guard = TEST_MUTEX.lock().await;
    let url = sms_test_support::database_url().await;

    let a = RoleLease::try_acquire(&url, Role::Hooks, "worker-a")
        .await
        .unwrap();
    let b = RoleLease::try_acquire(&url, Role::Jobs, "worker-a")
        .await
        .unwrap();

    assert!(a.is_some(), "one role's lock is independent of the other's");
    assert!(b.is_some(), "one role's lock is independent of the other's");
}

/// A provider `Role::Smpp` (this test's own role — see its own comment for
/// why it, specifically, is unused by the other four tests here) never
/// calls, since it stays `sms_worker::run`'s pure stub for the whole of this
/// crate's current milestone.
struct NeverCalledProvider;

#[async_trait::async_trait]
impl sms_provider::SmsProvider for NeverCalledProvider {
    fn key(&self) -> &str {
        unimplemented!("Role::Smpp's stub body never calls the provider")
    }
    fn capabilities(&self) -> sms_provider::Capabilities {
        unimplemented!("Role::Smpp's stub body never calls the provider")
    }
    async fn submit(
        &self,
        _req: &sms_provider::SubmitRequest,
    ) -> Result<sms_provider::SubmitAck, sms_provider::ProviderError> {
        unimplemented!("Role::Smpp's stub body never calls the provider")
    }
    fn parse_dlr(
        &self,
        _raw: &sms_provider::RawCallback,
    ) -> Result<Vec<sms_provider::DeliveryUpdate>, sms_provider::ProviderError> {
        unimplemented!("Role::Smpp's stub body never calls the provider")
    }
    async fn health(&self) -> sms_provider::Health {
        unimplemented!("Role::Smpp's stub body never calls the provider")
    }
}

/// #70's own "prove your guards can fail" standard, applied to the second
/// metric (`sms_sm001_total`, the first, is proven in `crates/sms-api/src/
/// errors.rs`'s own test). Drives the real `run_singleton` — not `RoleLease`
/// directly, the way every other test in this file does — because the
/// gauge is `run_singleton`'s own responsibility, not `RoleLease`'s: a bug
/// that regressed one of the `.set(0)`/`.set(1)` call sites in
/// `crates/sms-worker/src/lib.rs` would leave `RoleLease` itself completely
/// correct and this test is what would catch it.
///
/// `Role::Smpp`: the one role no other test in this file touches, so this
/// can run concurrently with the rest of the suite without contending on
/// the advisory lock itself — same reasoning
/// `different_roles_do_not_contend_with_each_other` gives for its own
/// choice of `Hooks`/`Jobs`.
#[tokio::test]
#[ignore = "needs a live Postgres — see module docs"]
async fn run_singleton_reports_held_then_released_on_the_lease_gauge() {
    let _guard = TEST_MUTEX.lock().await;
    let url = sms_test_support::database_url().await;

    let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
        .connect(&url)
        .await
        .expect("connecting the pool this test's own WorkerContext needs");
    let ctx = WorkerContext {
        db: sms_api::schema::Cratestack::builder(pool).build(),
        // #62 turned this into a registry keyed by `Provider.key`. Kept
        // populated rather than emptied: `NeverCalledProvider` panics if
        // anything submits through it, and this test drives `Role::Smpp`,
        // which must never dispatch. An empty registry would still pass —
        // by making a submit impossible rather than by proving none is
        // attempted — which is a weaker assertion than the one this
        // fixture was written to make.
        providers: std::sync::Arc::new(std::collections::HashMap::from([(
            "never_called".to_string(),
            std::sync::Arc::new(NeverCalledProvider)
                as std::sync::Arc<dyn sms_provider::SmsProvider>,
        )])),
    };

    let gauge = sms_metrics::SINGLETON_LEASE_HELD.with_label_values(&["smpp"]);

    let shutdown = tokio_util::sync::CancellationToken::new();
    let handle = tokio::spawn(sms_worker::run_singleton(
        Role::Smpp,
        url,
        ctx,
        "gauge-test-worker".to_owned(),
        shutdown.clone(),
    ));

    // Generous, not tuned: proving the gauge eventually reflects "held," not
    // measuring how fast `try_acquire` completes.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        gauge.get(),
        1,
        "run_singleton must report 1 once it holds the lease"
    );

    shutdown.cancel();
    handle
        .await
        .expect("run_singleton must return once cancelled, not panic");

    assert_eq!(
        gauge.get(),
        0,
        "run_singleton must report 0 again after releasing on shutdown"
    );
}
