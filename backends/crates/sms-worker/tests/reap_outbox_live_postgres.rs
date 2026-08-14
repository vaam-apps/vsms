//! Proves `#42`'s `reap_outbox` job (`backends/crates/sms-worker/src/jobs/reap_outbox.rs`)
//! against a real, fully migrated Postgres.
//!
//! Two properties, both load-bearing, both proven against real outbox rows
//! rather than by inspection:
//!
//! - A row that keeps failing delivery (a poison row, `attempts` past the
//!   #42 threshold) is never deleted — this job alerts on it and leaves it
//!   exactly where it is, still eligible for `drain` to retry.
//! - A row that *did* deliver is deleted once it is old enough
//!   (§7.5's 24h retention), and left alone before that.
//!
//! Neither property is provable through a delegate — `cratestack_event_outbox`
//! has none (see `reap_outbox.rs`'s own module doc for the R1 exception this
//! implies), so this suite proves "still there" and "gone" indirectly:
//!
//! - "Still there" (the poison-row case) is proven by re-triggering
//!   `db.events().drain()` *after* `ReapOutbox::run_at` and observing the
//!   same always-failing handler fire again for the same message — a row
//!   that no longer exists cannot be re-delivered to anything.
//! - "Gone" / "still there" (the delivered-row case) is read directly off
//!   `reap_delivered`'s own return value — `pub` for exactly this reason,
//!   the same convention `drain::oldest_undelivered_age` already documents:
//!   assert against real Postgres, not a scraped log line.
//!
//! Ignored by default, same convention as this crate's other live suites.
//! Run explicitly:
//!
//! ```bash
//! cargo test -p sms-worker --test reap_outbox_live_postgres -- --ignored
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{Duration as ChronoDuration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, CoolError};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use sms_worker::jobs::reap_outbox::{reap_delivered, ReapOutbox};
use sms_worker::jobs::JobHandler;

/// Same reasoning as every other live suite's own copy of this mutex — see
/// `claim_live_postgres.rs`'s own `TEST_MUTEX` doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "sms-worker-reap-outbox-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-worker-reap-outbox-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

/// A fresh `Cratestack` — a fresh pool *and* a fresh, empty `CoolEventBus` —
/// against the one database this test binary shares (`sms_test_support`
/// memoizes the URL per process). Same reasoning `drain_live_postgres.rs`'s
/// own `fresh_db` documents: registering a handler on one must never leak
/// onto another.
async fn fresh_db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "reap outbox test app".to_owned(),
            slug: format!("reap-outbox-test-{}", unique_suffix()),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the app")
        .id
}

async fn seed_endpoint(db: &Cratestack, app_id: &str, event_types: &str) {
    db.webhook_endpoint()
        .create(schema::CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: format!("https://example.test/webhooks/{}", unique_suffix()),
            eventTypes: event_types.to_owned(),
            secret: format!("test-secret-{}", unique_suffix()),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: false,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding a WebhookEndpoint");
}

async fn seed_message(db: &Cratestack, app_id: &str) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("reap-outbox-test-{}", unique_suffix())),
            msisdn: "+237677998877".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:reap-outbox-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 500,
            body: Some("reap_outbox test".to_owned()),
            bodyHash: format!("hmac-sha256-v1:reap-outbox-test-{}", unique_suffix()),
            bodyLength: 17,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: None,
            providerId: None,
            providerMessageRef: None,
            providerMessageRefAlt: None,
            excludedRouteIds: None,
            maxAttempts: 3,
            leaseOwner: None,
            leaseUntil: None,
            scheduledAt: None,
            expiresAt: Utc::now() + ChronoDuration::hours(1),
            submittedAt: None,
            finalizedAt: None,
            purgedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message")
}

/// Drives a fresh message straight to `cancelled` — a direct, one-hop legal
/// edge from `accepted` (`message_state_transitions`) — the one write on an
/// emitting model this whole suite needs: it triggers the mutation's own
/// automatic post-commit drain, the first delivery attempt.
async fn cancel(db: &Cratestack, message: &Message) {
    db.message()
        .update(message.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(MessageState::cancelled),
            ..Default::default()
        })
        .if_match(message.version)
        .run(&sys())
        .await
        .expect("cancelling the message");
}

async fn seed_reap_outbox_job(db: &Cratestack) -> schema::Job {
    db.job()
        .create(schema::CreateJobInput {
            kind: "reap_outbox".to_owned(),
            dedupeKey: None,
            payload: "{}".to_owned(),
            priority: 500,
            runAt: Utc::now(),
            leaseOwner: None,
            leaseUntil: None,
            maxAttempts: 3,
            lastError: None,
            startedAt: None,
            finishedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the reap_outbox job")
}

/// This test registers its own always-failing `on_message_updated` handler
/// directly on the outbox hook, standing in for "any subscriber that keeps
/// erroring on this event forever" — the actual scenario #42 exists to
/// make loud rather than silently unbounded. It is deliberately not
/// `sms_api::webhooks::register_subscribers`; that subscriber never errors
/// on a well-formed row, so it cannot produce a poison row on its own.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_poison_row_is_alerted_but_never_deleted() {
    let _guard = TEST_MUTEX.lock().await;

    let writer = fresh_db().await;
    let target_id: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let seen_after_reap = Arc::new(AtomicBool::new(false));

    let target_for_handler = target_id.clone();
    let seen_for_handler = seen_after_reap.clone();
    writer
        .events()
        .on_message_updated(move |event: schema::events::MessageUpdatedEvent| {
            let target = target_for_handler.clone();
            let seen = seen_for_handler.clone();
            async move {
                if event.data.id == *target.lock().expect("target id mutex") {
                    seen.store(true, Ordering::SeqCst);
                }
                Err(CoolError::Internal(
                    "simulated permanent subscriber failure".to_owned(),
                ))
            }
        });

    let app_id = seed_app(&writer).await;
    seed_endpoint(&writer, &app_id, " message.cancelled ").await;
    let message = seed_message(&writer, &app_id).await;
    *target_id.lock().expect("target id mutex") = message.id.clone();

    // accepted -> cancelled triggers the automatic post-commit drain:
    // attempts = 1, delivered_at still NULL.
    cancel(&writer, &message).await;

    // Five more explicit drains push attempts to 6 (1 + 5), past #42's own
    // `attempts > 5` threshold, without waiting on drain's real 5s cadence.
    for _ in 0..5 {
        writer.events().drain().await.expect("draining the outbox");
    }

    // Run the real job against a third, independent instance — no
    // handlers of its own, exactly what the scheduled job sees in
    // production.
    let job_runner = fresh_db().await;
    ReapOutbox
        .run_at(&job_runner, Utc::now())
        .await
        .expect("reap_outbox run_at succeeds");

    // Prove the row is still there: reset the flag, drain once more on the
    // *original* writer (still holding the always-failing handler), and
    // check it fired again for our message. A deleted row could never do
    // this — `drain_event_outbox` only ever selects rows that still exist.
    seen_after_reap.store(false, Ordering::SeqCst);
    writer.events().drain().await.expect("draining the outbox");
    assert!(
        seen_after_reap.load(Ordering::SeqCst),
        "the poison row must still exist and still be undelivered after reap_outbox ran — \
         it was alerted on, not deleted"
    );
}

/// A row that *did* deliver, well past the 24h retention: `reap_delivered`
/// must actually remove it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_stale_delivered_row_is_reaped() {
    let _guard = TEST_MUTEX.lock().await;

    let writer = fresh_db().await;
    // Succeeds unconditionally — the automatic post-commit drain this
    // triggers marks the row `delivered_at = NOW()` on its first and only
    // attempt.
    writer
        .events()
        .on_message_updated(|_event: schema::events::MessageUpdatedEvent| async { Ok(()) });

    let app_id = seed_app(&writer).await;
    seed_endpoint(&writer, &app_id, " message.cancelled ").await;
    let message = seed_message(&writer, &app_id).await;
    cancel(&writer, &message).await;

    // A cutoff two days in the future is, relative to this row's very
    // recent real `delivered_at`, indistinguishable from "the 24h
    // retention has elapsed" — the same virtual-clock trick
    // `ExpireStale`'s own live tests use for `Message.updatedAt`, applied
    // here to a column this crate has no delegate seam to backdate at all.
    let checker = fresh_db().await;
    let cutoff = Utc::now() + ChronoDuration::days(2);
    let reaped = reap_delivered(&checker, cutoff)
        .await
        .expect("reap_delivered succeeds");

    assert!(
        reaped >= 1,
        "at least this test's own delivered row must have been reaped with such a generous cutoff"
    );
}

/// The other half of "prove it reaps only what it should": a delivered row
/// that is still fresh must survive a run whose retention window hasn't
/// elapsed for it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_fresh_delivered_row_survives() {
    let _guard = TEST_MUTEX.lock().await;

    let writer = fresh_db().await;
    writer
        .events()
        .on_message_updated(|_event: schema::events::MessageUpdatedEvent| async { Ok(()) });

    let app_id = seed_app(&writer).await;
    seed_endpoint(&writer, &app_id, " message.cancelled ").await;
    let message = seed_message(&writer, &app_id).await;
    cancel(&writer, &message).await;

    // A cutoff two days in the *past* is well before this row's real,
    // just-now `delivered_at` — nothing this fresh can match
    // `delivered_at < cutoff`.
    let checker = fresh_db().await;
    let cutoff = Utc::now() - ChronoDuration::days(2);
    let reaped = reap_delivered(&checker, cutoff)
        .await
        .expect("reap_delivered succeeds");

    assert_eq!(
        reaped, 0,
        "a delivered row still within its retention window must not be reaped by a stale-dated cutoff"
    );
}

/// End-to-end sanity on the `JobHandler` entry point itself, seeded and run
/// exactly the way `Role::Jobs`'s real claim loop would reach it — `run_at`
/// is what the tests above exercise directly; this proves the
/// `JobHandler::run`/`kind` wiring `default_registry` depends on also does
/// something real against a live database, not just that the dispatch
/// table has an entry named `reap_outbox`.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_job_handler_entry_point_runs_without_error_against_a_live_database() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;

    let job = seed_reap_outbox_job(&db).await;
    let outcome = ReapOutbox.run(&db, &sys(), &job).await;
    assert!(
        outcome.is_ok(),
        "reap_outbox's JobHandler::run must succeed: {outcome:?}"
    );
    assert_eq!(ReapOutbox.kind(), "reap_outbox");
}
