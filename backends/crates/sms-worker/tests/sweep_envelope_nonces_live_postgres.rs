//! Proves `sweep_envelope_nonces` (VS-3) against a real Postgres: it deletes
//! the replay cache's expired rows, keeps the live ones, and clears a backlog
//! larger than one batch over a single run.
//!
//! Rows are written through the generated delegate with the `expiresAt` the
//! test wants (the column is a plain input, not defaulted), so the clock is
//! real data and `run_at` is given a fixed `now`.
//!
//! Ignored by default, same convention as this crate's other live suites:
//!
//! ```bash
//! cargo test -p sms-worker --test sweep_envelope_nonces_live_postgres -- --ignored
//! ```

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sms_api::schema::{self, Cratestack, envelope_nonce};
use sms_worker::jobs::sweep_envelope_nonces::SweepEnvelopeNonces;

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CratestackContext {
    sms_api::auth::system_context("sms-worker-sweep-envelope-nonces-test")
}

async fn fresh_db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

fn tag() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()
}

async fn seed(db: &Cratestack, nonce: String, expires_in: Duration) {
    db.envelope_nonce()
        .create(schema::CreateEnvelopeNonceInput {
            nonce,
            expiresAt: Utc::now() + expires_in,
        })
        .run(&sys())
        .await
        .expect("seeding an EnvelopeNonce");
}

async fn count_with_prefix(db: &Cratestack, prefix: &str) -> usize {
    db.envelope_nonce()
        .find_many()
        .where_expr(FilterExpr::from(
            envelope_nonce::nonce().starts_with(prefix.to_owned()),
        ))
        .limit(5000)
        .run(&sys())
        .await
        .expect("counting nonces")
        .len()
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn expired_nonces_are_swept_and_live_ones_are_kept() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let prefix = format!("cose:sweep-{}:", tag());

    seed(&db, format!("{prefix}expired-a"), Duration::minutes(-1)).await;
    seed(&db, format!("{prefix}expired-b"), Duration::hours(-3)).await;
    seed(&db, format!("{prefix}live"), Duration::minutes(10)).await;

    SweepEnvelopeNonces
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("the sweep runs");

    assert_eq!(
        count_with_prefix(&db, &prefix).await,
        1,
        "only the live nonce survives"
    );
}

/// The boundary a replay cache must get right: a nonce is kept up to and not
/// past its `expiresAt`, judged on the clock the sweep is given.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_nonce_is_kept_until_its_own_expiry_on_the_sweeps_clock() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let prefix = format!("cose:sweep-{}:", tag());
    seed(&db, format!("{prefix}x"), Duration::minutes(10)).await;

    SweepEnvelopeNonces
        .run_at(&db, &sys(), Utc::now() + Duration::minutes(9))
        .await
        .unwrap();
    assert_eq!(count_with_prefix(&db, &prefix).await, 1, "not yet expired");

    SweepEnvelopeNonces
        .run_at(&db, &sys(), Utc::now() + Duration::minutes(11))
        .await
        .unwrap();
    assert_eq!(count_with_prefix(&db, &prefix).await, 0, "expired, swept");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_backlog_larger_than_one_batch_is_cleared_in_one_run() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let prefix = format!("cose:sweep-{}:", tag());
    for i in 0..620 {
        seed(&db, format!("{prefix}{i}"), Duration::minutes(-5)).await;
    }

    let deleted = SweepEnvelopeNonces
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("the sweep runs");

    assert!(deleted >= 620, "deleted {deleted}");
    assert_eq!(count_with_prefix(&db, &prefix).await, 0);
}
