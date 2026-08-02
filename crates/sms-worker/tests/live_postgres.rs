//! Proves [`RoleLease`] against a real Postgres — in particular, that
//! dropping a lease without calling `release()` still frees the lock. That's
//! the entire reason this module exists instead of the §7.2 sample code's
//! `pool.acquire()` shape; a unit test can't demonstrate it since it needs a
//! second, independent connection to observe the lock's state from outside.
//!
//! Ignored by default, same convention as `sms-auth`'s live suite — `just
//! test` carries no `DATABASE_URL`. Run explicitly:
//!
//! ```bash
//! docker run --rm -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:16
//! createdb vsms_check
//! DATABASE_URL=postgres://postgres:postgres@localhost/vsms_check \
//!     cargo test -p sms-worker --test live_postgres -- --ignored
//! ```
//!
//! No migration needed — advisory locks aren't backed by any table this
//! schema declares, so an empty freshly-created database is enough.

use sms_worker::lease::RoleLease;
use sms_worker::Role;
use std::time::Duration;

fn database_url() -> String {
    std::env::var("DATABASE_URL").expect("DATABASE_URL must point at a reachable Postgres")
}

#[tokio::test]
#[ignore = "needs a live Postgres — see module docs"]
async fn a_second_attempt_for_the_same_role_is_none_while_the_first_holds_it() {
    let url = database_url();

    let first = RoleLease::try_acquire(&url, Role::Dispatch)
        .await
        .expect("first attempt succeeds")
        .expect("first attempt is the winner");

    let second = RoleLease::try_acquire(&url, Role::Dispatch)
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
    let url = database_url();

    let first = RoleLease::try_acquire(&url, Role::Drain)
        .await
        .unwrap()
        .expect("first attempt is the winner");
    first.release().await.expect("releasing");

    let second = RoleLease::try_acquire(&url, Role::Drain).await.unwrap();
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
    let url = database_url();

    let first = RoleLease::try_acquire(&url, Role::Scheduler)
        .await
        .unwrap()
        .expect("first attempt is the winner");

    drop(first); // no .release() — simulating a panic or a `kill -9`

    // Dropping closes the socket, but the close is not instant from this
    // task's point of view (no `.await` happens in `Drop`) — give Postgres a
    // moment to notice the session ended before asserting on it. Generous on
    // purpose: this is proving a property, not measuring latency.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let second = RoleLease::try_acquire(&url, Role::Scheduler).await.unwrap();
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
    let url = database_url();

    let a = RoleLease::try_acquire(&url, Role::Hooks).await.unwrap();
    let b = RoleLease::try_acquire(&url, Role::Jobs).await.unwrap();

    assert!(a.is_some(), "one role's lock is independent of the other's");
    assert!(b.is_some(), "one role's lock is independent of the other's");
}
