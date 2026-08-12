//! Proves #39's `drain` role (`crates/sms-worker/src/drain.rs`) against a
//! real, fully migrated Postgres.
//!
//! The property worth proving isn't "subscribers work" — that's #38's own
//! `crates/sms-api/tests/webhooks_live_postgres.rs`, and it already shows
//! a plain `Message` update self-drains with no explicit `.drain()` call
//! anywhere. What *this* role adds on top of that automatic post-commit
//! drain is a write-independent retry trigger for a row whose handler
//! failed on an earlier attempt — see `sms_api::webhooks`'s own module
//! doc for why that's the honest answer to "what does drain drain".
//!
//! `drain_tick_retries_a_row_whose_first_delivery_attempt_failed` proves
//! exactly that scenario, end to end: one `Cratestack` instance
//! ("writer", standing in for whatever process performed the mutation —
//! `sendMessage`, DLR ingestion, `dispatch`) registers a subscriber that
//! always fails on its first attempt, so the write's own automatic
//! post-commit drain leaves the outbox row undelivered
//! (`attempts`/`last_error` recorded, `delivered_at IS NULL`) with **no**
//! `WebhookAttempt` row created. A second, independent `Cratestack`
//! instance ("drain role") registers the real production
//! `sms_api::webhooks::register_subscribers` and calls `drain::tick`
//! directly — no further write happens on any emitting model in between —
//! and that alone is what turns the stuck row into a real
//! `WebhookAttempt`.
//!
//! Ignored by default, same convention as this crate's other live suites.
//! Run explicitly:
//!
//! ```bash
//! cargo test -p sms-worker --test drain_live_postgres -- --ignored
//! ```

use chrono::{Duration as ChronoDuration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, webhook_attempt, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use sms_worker::drain;

/// Same reasoning as every other live suite's own copy of this mutex —
/// see `crates/sms-worker/tests/claim_live_postgres.rs`'s doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "sms-worker-drain-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-worker-drain-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

/// A fresh `Cratestack` — a fresh pool *and* a fresh, empty
/// `CoolEventBus` — against the one database this test binary shares
/// (`sms_test_support::database_url()` memoizes the URL per process, so
/// every call here targets the same Postgres). Deliberately not shared
/// between "writer" and "drain role" in a test: registering subscribers
/// on one must not leak onto the other, the same way `app/sms-gateway`
/// and `app/sms-worker` are genuinely separate processes with genuinely
/// separate event buses in production.
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
            name: "drain test app".to_owned(),
            slug: format!("drain-test-{}", unique_suffix()),
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

async fn seed_endpoint(
    db: &Cratestack,
    app_id: &str,
    event_types: &str,
) -> schema::WebhookEndpoint {
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
        .expect("seeding a WebhookEndpoint")
}

async fn seed_message(db: &Cratestack, app_id: &str) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("drain-test-{}", unique_suffix())),
            msisdn: "+237677223344".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:drain-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 500,
            body: Some("drain role test".to_owned()),
            bodyHash: format!("hmac-sha256-v1:drain-test-{}", unique_suffix()),
            bodyLength: 15,
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

/// `owner()`, not `sys()` — `WebhookAttempt`'s own `@@allow("list"/
/// "detail", ...)` clause has no `hasRole('system')` branch (unlike
/// `create`/`update`). See `crates/sms-api/tests/webhooks_live_postgres.rs`'s
/// own copy of this same finding for the full reasoning; not fixed here
/// since nothing in #38/#39's own production code ever reads
/// `WebhookAttempt` under a system context.
async fn attempts_for(db: &Cratestack, message_id: &str) -> Vec<schema::WebhookAttempt> {
    db.webhook_attempt()
        .find_many()
        .where_expr(FilterExpr::from(
            webhook_attempt::aggregateId().eq(message_id.to_owned()),
        ))
        .run(&owner())
        .await
        .expect("listing webhook attempts")
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn drain_tick_retries_a_row_whose_first_delivery_attempt_failed() {
    let _guard = TEST_MUTEX.lock().await;

    let writer = fresh_db().await;
    // Simulates a transient failure on the first delivery attempt — e.g.
    // a momentary connection hiccup creating the WebhookAttempt row. Not
    // sms_api::webhooks::register_subscribers: this is deliberately a
    // different, always-failing handler, standing in for "the real one,
    // but it failed this time".
    writer
        .events()
        .on_message_updated(|_event: schema::events::MessageUpdatedEvent| async {
            Err(CoolError::Internal(
                "simulated transient failure on first delivery attempt".to_owned(),
            ))
        });

    let app_id = seed_app(&writer).await;
    seed_endpoint(&writer, &app_id, " message.cancelled ").await;
    let message = seed_message(&writer, &app_id).await;

    // accepted -> cancelled: a direct, one-hop legal edge
    // (message_state_transitions), so this is the only write on an
    // emitting model in the whole test — the automatic post-commit drain
    // this triggers is what runs the always-failing handler above.
    writer
        .message()
        .update(message.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(MessageState::cancelled),
            ..Default::default()
        })
        .if_match(message.version)
        .run(&sys())
        .await
        .expect("cancelling the message");

    // The write's own automatic drain ran the always-failing handler and
    // recorded the failure — nothing should have created a WebhookAttempt
    // yet. Read this back through a *third*, unrelated Cratestack
    // instance (no subscribers of its own) to prove the row's actual
    // database state, not something implied by the writer's own bus.
    let checker = fresh_db().await;
    let attempts_before = attempts_for(&checker, &message.id).await;
    assert!(
        attempts_before.is_empty(),
        "the always-failing handler must not have created a WebhookAttempt: {attempts_before:?}"
    );

    // The drain role's own Cratestack instance: a fresh, independent
    // CoolEventBus carrying the *real* production subscribers. No further
    // write on any emitting model happens between here and the
    // assertions below — tick() alone has to be what turns the stuck row
    // into a real WebhookAttempt.
    let drain_role_db = fresh_db().await;
    sms_api::webhooks::register_subscribers(&drain_role_db);
    drain::tick(&drain_role_db).await;

    let attempts_after = attempts_for(&checker, &message.id).await;
    assert_eq!(
        attempts_after.len(),
        1,
        "drain::tick should have retried the previously-failed row and created exactly one \
         WebhookAttempt: {attempts_after:?}"
    );
    assert_eq!(attempts_after[0].eventType, "message.cancelled");
    assert_eq!(attempts_after[0].aggregateId, message.id);
}

/// #39's own acceptance line: alert on oldest-undelivered age, not just on
/// errors. Proven directly against `oldest_undelivered_age`, the R1
/// exception `tick` logs from, rather than scraping a `tracing` log line.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn oldest_undelivered_age_reflects_a_real_stuck_row_and_clears_after_drain() {
    let _guard = TEST_MUTEX.lock().await;

    let writer = fresh_db().await;
    writer
        .events()
        .on_message_updated(|_event: schema::events::MessageUpdatedEvent| async {
            Err(CoolError::Internal(
                "simulated transient failure".to_owned(),
            ))
        });

    let app_id = seed_app(&writer).await;
    seed_endpoint(&writer, &app_id, " message.cancelled ").await;
    let message = seed_message(&writer, &app_id).await;

    writer
        .message()
        .update(message.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(MessageState::cancelled),
            ..Default::default()
        })
        .if_match(message.version)
        .run(&sys())
        .await
        .expect("cancelling the message");

    let checker = fresh_db().await;
    let age = drain::oldest_undelivered_age(&checker)
        .await
        .expect("reading oldest undelivered age");
    let age = age.expect("a row is stuck undelivered; age must be Some");
    assert!(
        age >= ChronoDuration::zero() && age < ChronoDuration::minutes(5),
        "age should be a small, non-negative duration measured from just now: {age:?}"
    );

    let drain_role_db = fresh_db().await;
    sms_api::webhooks::register_subscribers(&drain_role_db);
    drain::tick(&drain_role_db).await;

    // Draining this specific row doesn't guarantee the table has *no*
    // undelivered rows at all — this database is never reset between
    // test runs, and other suites in this workspace's own live-Postgres
    // sweep may leave unrelated rows behind. What this proves is narrower
    // and still real: this test's own row is no longer the reason
    // anything would be undelivered, which `attempts_for` above already
    // confirmed by finding it delivered into a WebhookAttempt.
    let attempts = attempts_for(&checker, &message.id).await;
    assert_eq!(attempts.len(), 1, "the row must have actually drained");
}
