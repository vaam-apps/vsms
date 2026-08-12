//! Proves `#67`'s `purge_retention` job
//! (`crates/sms-worker/src/jobs/purge_retention.rs`) against a real, fully
//! migrated Postgres.
//!
//! The acceptance criterion this issue names explicitly: verify against a
//! **seeded old dataset**, not fresh data no purge rule matches. A test that
//! inserts a row and immediately asserts nothing was purged proves nothing.
//!
//! # How rows are seeded genuinely old — two different mechanisms, on purpose
//!
//! `Message.createdAt` and `DeliveryReceipt.receivedAt` both carry
//! `@default(dbgenerated())`, which excludes them from their own create
//! input (§2.0) — neither can be set at row-creation time. But they are not
//! symmetric past that point:
//!
//! - **`Message.createdAt` genuinely can be backdated through a delegate.**
//!   `@default(...)` excludes a field from `CreateXInput` but not
//!   `UpdateXInput` (AGENTS.md's own framework-constraints table: "Defaulted
//!   fields *are* settable on update"), and — unlike `updatedAt` — no
//!   trigger touches `created_at` on write (`0002_bootstrap`'s
//!   `touch_updated_at` only ever sets `updated_at`; confirmed by reading
//!   the trigger function itself, not assumed from the mixin's name). So
//!   every boundary test below seeds a message, drives it to a terminal
//!   state, and backdates `createdAt` to a real, past `DateTime<Utc>` in the
//!   *same* `UpdateMessageInput` write — genuinely old data, not a virtual
//!   clock.
//! - **`DeliveryReceipt.receivedAt` cannot be backdated at all** —
//!   `DeliveryReceipt` has no `@@allow("update", ...)` clause whatsoever
//!   (append-only by design; `crates/sms-api/src/dlr.rs` only ever
//!   `.create()`s one), so there is no delegate seam to reach it through,
//!   genuinely or otherwise. The receipt tests below use the same
//!   virtual-`cutoff` trick `reap_outbox_live_postgres.rs` already
//!   establishes for exactly this shape of column (framework/DB-stamped, no
//!   update capability): seed a real, fresh receipt, then shift the
//!   `cutoff` argument instead of the row's own timestamp. `PurgeRetention`
//!   is deliberately structured so its two halves can be driven
//!   independently for this reason — see `run_at`'s own two calls.
//!
//! # The webhook-suppression guard
//!
//! `a_purge_never_re_fires_a_webhook_to_a_registered_endpoint` proves the
//! fix for a real bug the coordinator found reviewing this PR:
//! `purge_messages`'s own `.update()` is a real write against a model with
//! `@@emit(created, updated)`, and `crates/sms-api/src/webhooks.rs`'s
//! subscriber (`enqueue_message_webhook_attempts`) has, since that fix, an
//! explicit `message.purgedAt.is_some()` guard — before it, four of this
//! job's five terminal candidate states mapped to a catalogued event, so a
//! purge would have enqueued (and `hooks` would then have signed and
//! `POSTed`) a live webhook about a message three months stale. The test
//! deliberately builds the one case `webhook_attempts_dedupe`'s unique
//! index cannot save: a `WebhookEndpoint` created *after* the message
//! already reached `delivered`, so there is no prior attempt for a
//! purge-triggered one to collide with — if the guard in `webhooks.rs`
//! were ever removed, this is the test that would catch it, not dedupe.
//!
//! Ignored by default, same convention as this crate's other live suites.
//! Run explicitly:
//!
//! ```bash
//! cargo test -p sms-worker --test purge_retention_live_postgres -- --ignored
//! ```

use chrono::{Duration as ChronoDuration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::schema::{
    self, delivery_receipt, message, webhook_attempt, Cratestack, DeliveryOutcome, Encoding,
    Message, MessageClass, MessageState, OperatorCode, UpdateMessageInput,
};
use sms_worker::jobs::purge_retention::PurgeRetention;
use sms_worker::jobs::JobHandler;

/// Same reasoning as every other live suite's own copy of this mutex — see
/// `claim_live_postgres.rs`'s own `TEST_MUTEX` doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    sms_api::auth::Principal {
        sub: "sms-worker-purge-retention-test".to_owned(),
        kind: sms_api::auth::PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    sms_api::auth::Principal {
        sub: "sms-worker-purge-retention-test-owner".to_owned(),
        kind: sms_api::auth::PrincipalKind::User,
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

/// A fresh `Cratestack` against the one database this test binary shares
/// (`sms_test_support` memoizes the URL per process) — same convention
/// `reap_outbox_live_postgres.rs`'s own `fresh_db` documents.
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
            name: "purge retention test app".to_owned(),
            slug: format!("purge-retention-test-{}", unique_suffix()),
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

async fn seed_message(db: &Cratestack, app_id: &str) -> Message {
    let suffix = unique_suffix();
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: Some(format!("order-{suffix}")),
            idempotencyKey: Some(format!("order-{suffix}")),
            msisdn: "+237677998877".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:purge-retention-test-{suffix}"),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 500,
            body: Some("Votre code est 4821".to_owned()),
            bodyHash: format!("hmac-sha256-v1:purge-retention-test-body-{suffix}"),
            bodyLength: 20,
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
/// edge from `accepted` (`message_state_transitions`), same helper shape
/// `reap_outbox_live_postgres.rs`'s own `cancel` uses — *and*, in the same
/// write, backdates `createdAt` to `created_at`. One CAS write, genuinely
/// old data: see the module doc for why this is possible for `Message` and
/// not for `DeliveryReceipt`.
async fn seed_terminal_message(
    db: &Cratestack,
    app_id: &str,
    created_at: chrono::DateTime<Utc>,
) -> Message {
    let message = seed_message(db, app_id).await;
    db.message()
        .update(message.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::cancelled),
            createdAt: Some(created_at),
            ..Default::default()
        })
        .if_match(message.version)
        .run(&sys())
        .await
        .expect("driving the message to cancelled and backdating createdAt")
}

async fn seed_receipt(db: &Cratestack, message_id: &str) {
    db.delivery_receipt()
        .create(schema::CreateDeliveryReceiptInput {
            messageId: message_id.to_owned(),
            providerId: "orange-cm".to_owned(),
            providerMessageRef: format!("orange-ref-{}", unique_suffix()),
            outcome: DeliveryOutcome::delivered,
            rawStatus: "DeliveredToTerminal".to_owned(),
            errorCode: None,
            networkCode: OperatorCode::mtn,
            occurredAt: Some(Utc::now()),
            rawPayload: r#"{"status":"DeliveredToTerminal"}"#.to_owned(),
        })
        .run(&sys())
        .await
        .expect("seeding a delivery receipt");
}

async fn reload_message(db: &Cratestack, id: &str) -> Message {
    db.message()
        .find_many()
        .where_expr(FilterExpr::from(message::id().eq(id.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("reloading the message")
        .into_iter()
        .next()
        .expect("the message must still exist")
}

async fn count_receipts_for(db: &Cratestack, message_id: &str) -> usize {
    db.delivery_receipt()
        .find_many()
        .where_expr(FilterExpr::from(
            delivery_receipt::messageId().eq(message_id.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("counting receipts")
        .len()
}

/// The core positive case: a terminal message comfortably past the 90-day
/// window is purged — every documented column, not just one.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_terminal_message_comfortably_past_retention_is_purged() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let app_id = seed_app(&db).await;

    let old = Utc::now() - ChronoDuration::days(100);
    let seeded = seed_terminal_message(&db, &app_id, old).await;

    PurgeRetention
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("purge_retention run_at succeeds");

    let purged = reload_message(&db, &seeded.id).await;
    assert_eq!(purged.msisdn, "purged-msisdn");
    assert_eq!(purged.body, None);
    assert_eq!(purged.clientRef, None);
    assert_eq!(purged.idempotencyKey, None);
    assert_eq!(purged.stateReason, None);
    assert!(purged.purgedAt.is_some(), "purgedAt must be stamped");

    // What must survive: the correlation key the decision's own text names
    // explicitly, and everything that isn't recipient PII.
    assert_eq!(purged.msisdnHash, seeded.msisdnHash);
    assert_eq!(purged.bodyHash, seeded.bodyHash);
    assert_eq!(purged.senderIdValue, seeded.senderIdValue);
    assert_eq!(purged.state, MessageState::cancelled);
}

/// The boundary, all four points the issue's own acceptance criterion asks
/// for: comfortably past, just past, exactly at, just inside, comfortably
/// inside. `RETENTION` is 90 days and the filter is `createdAt <= cutoff`
/// (`cutoff = now - 90d`), so "exactly at" must purge (inclusive) and "just
/// inside" must survive.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_90_day_boundary_is_inclusive_and_exact() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let app_id = seed_app(&db).await;

    let now = Utc::now();
    let retention = ChronoDuration::days(90);

    let comfortably_past =
        seed_terminal_message(&db, &app_id, now - retention - ChronoDuration::days(10)).await;
    let just_past =
        seed_terminal_message(&db, &app_id, now - retention - ChronoDuration::seconds(5)).await;
    let exactly_at = seed_terminal_message(&db, &app_id, now - retention).await;
    let just_inside =
        seed_terminal_message(&db, &app_id, now - retention + ChronoDuration::seconds(5)).await;
    let comfortably_inside =
        seed_terminal_message(&db, &app_id, now - ChronoDuration::days(1)).await;

    PurgeRetention
        .run_at(&db, &sys(), now)
        .await
        .expect("purge_retention run_at succeeds");

    let cases = [
        ("comfortably_past", &comfortably_past, true),
        ("just_past", &just_past, true),
        ("exactly_at", &exactly_at, true),
        ("just_inside", &just_inside, false),
        ("comfortably_inside", &comfortably_inside, false),
    ];

    for (label, seeded, should_be_purged) in cases {
        let reloaded = reload_message(&db, &seeded.id).await;
        assert_eq!(
            reloaded.purgedAt.is_some(),
            should_be_purged,
            "{label}: purgedAt.is_some() expected {should_be_purged}, createdAt={:?}, cutoff={:?}",
            seeded.createdAt,
            now - retention,
        );
        assert_eq!(
            reloaded.msisdn == "purged-msisdn",
            should_be_purged,
            "{label}: msisdn purge state mismatch"
        );
    }
}

/// A non-terminal message past 90 days must never be touched, even though
/// its `createdAt` alone would match — the state filter is what makes this
/// job trust §7.4's own expiry guarantee rather than forcing a still-live
/// row over it. See the module doc's own reasoning.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_non_terminal_message_past_retention_is_left_alone() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let app_id = seed_app(&db).await;

    let seeded = seed_message(&db, &app_id).await;
    let old = Utc::now() - ChronoDuration::days(200);
    db.message()
        .update(seeded.id.clone())
        .set(UpdateMessageInput {
            createdAt: Some(old),
            ..Default::default()
        })
        .if_match(seeded.version)
        .run(&sys())
        .await
        .expect("backdating createdAt on an accepted message");

    PurgeRetention
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("purge_retention run_at succeeds");

    let reloaded = reload_message(&db, &seeded.id).await;
    assert_eq!(reloaded.state, MessageState::accepted);
    assert!(
        reloaded.purgedAt.is_none(),
        "a non-terminal message must never be purged regardless of age"
    );
    assert_eq!(reloaded.msisdn, "+237677998877");
}

/// Idempotency: running the job twice against the same already-purged row
/// must not error, and must not touch it again — proven by asserting the
/// row's `version` (bumped on every real write) stops moving after the
/// first run.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_second_run_does_not_re_purge_an_already_purged_message() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let app_id = seed_app(&db).await;

    let old = Utc::now() - ChronoDuration::days(120);
    let seeded = seed_terminal_message(&db, &app_id, old).await;

    PurgeRetention
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("first purge_retention run_at succeeds");
    let after_first = reload_message(&db, &seeded.id).await;
    assert!(after_first.purgedAt.is_some());

    PurgeRetention
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("second purge_retention run_at succeeds");
    let after_second = reload_message(&db, &seeded.id).await;

    assert_eq!(
        after_first.version, after_second.version,
        "a second run must not write an already-purged row again"
    );
    assert_eq!(after_first.purgedAt, after_second.purgedAt);
}

/// `DeliveryReceipt` half: a receipt past its own 90-day `receivedAt` is
/// deleted. Uses the virtual-cutoff trick (see the module doc) since
/// `receivedAt` cannot be backdated through any delegate.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_stale_receipt_is_deleted() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let app_id = seed_app(&db).await;
    let message = seed_message(&db, &app_id).await;
    seed_receipt(&db, &message.id).await;

    assert_eq!(count_receipts_for(&db, &message.id).await, 1);

    // A `now` 91 days in the future is, relative to this row's very recent
    // real `receivedAt`, indistinguishable from "90 days have elapsed" —
    // the same trick `reap_outbox_live_postgres.rs` already uses for
    // `delivered_at`.
    let virtual_now = Utc::now() + ChronoDuration::days(91);
    PurgeRetention
        .run_at(&db, &sys(), virtual_now)
        .await
        .expect("purge_retention run_at succeeds");

    assert_eq!(
        count_receipts_for(&db, &message.id).await,
        0,
        "a receipt past its own 90-day retention must be deleted"
    );
}

/// The other half of "prove it deletes only what it should": a fresh
/// receipt survives a run whose retention window hasn't elapsed for it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_fresh_receipt_survives() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let app_id = seed_app(&db).await;
    let message = seed_message(&db, &app_id).await;
    seed_receipt(&db, &message.id).await;

    PurgeRetention
        .run_at(&db, &sys(), Utc::now())
        .await
        .expect("purge_retention run_at succeeds");

    assert_eq!(
        count_receipts_for(&db, &message.id).await,
        1,
        "a fresh receipt well within its retention window must survive"
    );
}

/// End-to-end sanity on the `JobHandler` entry point itself, seeded and run
/// exactly the way `Role::Jobs`'s real claim loop would reach it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_job_handler_entry_point_runs_without_error_against_a_live_database() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;

    let job = db
        .job()
        .create(schema::CreateJobInput {
            kind: "purge_retention".to_owned(),
            dedupeKey: None,
            payload: "{}".to_owned(),
            priority: 100,
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
        .expect("seeding the purge_retention job");

    let outcome = PurgeRetention.run(&db, &sys(), &job).await;
    assert!(
        outcome.is_ok(),
        "purge_retention's JobHandler::run must succeed: {outcome:?}"
    );
    assert_eq!(PurgeRetention.kind(), "purge_retention");
}

async fn seed_webhook_endpoint(db: &Cratestack, app_id: &str, event_types: &str) {
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

async fn count_attempts_for(db: &Cratestack, message_id: &str) -> usize {
    db.webhook_attempt()
        .find_many()
        .where_expr(FilterExpr::from(
            webhook_attempt::aggregateId().eq(message_id.to_owned()),
        ))
        .run(&sys())
        .await
        .expect("counting webhook attempts")
        .len()
}

/// Drives a fresh message all the way to `delivered` — `accepted -> queued
/// -> routed -> submitted -> delivered`, each a real, separate, legal hop
/// (`accepted -> delivered` directly is not in `message_state_transitions`
/// — same fact `errors_live_postgres.rs`'s own
/// `an_illegal_transition_surfaces_as_409_not_500` test asserts). The final
/// hop also backdates `createdAt`, in the same write, so the row is both
/// terminal and old by the time this returns — see the module doc's own
/// backdating reasoning.
async fn drive_to_delivered(
    db: &Cratestack,
    app_id: &str,
    created_at: chrono::DateTime<Utc>,
) -> Message {
    let message = seed_message(db, app_id).await;

    let queued = db
        .message()
        .update(message.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::queued),
            providerId: Some(Some("orange-cm".to_owned())),
            ..Default::default()
        })
        .if_match(message.version)
        .run(&sys())
        .await
        .expect("accepted -> queued");

    let routed = db
        .message()
        .update(queued.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::routed),
            ..Default::default()
        })
        .if_match(queued.version)
        .run(&sys())
        .await
        .expect("queued -> routed");

    let submitted = db
        .message()
        .update(routed.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::submitted),
            providerMessageRef: Some(Some(format!("orange-ref-{}", unique_suffix()))),
            ..Default::default()
        })
        .if_match(routed.version)
        .run(&sys())
        .await
        .expect("routed -> submitted");

    db.message()
        .update(submitted.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::delivered),
            createdAt: Some(created_at),
            ..Default::default()
        })
        .if_match(submitted.version)
        .run(&sys())
        .await
        .expect("submitted -> delivered, backdating createdAt")
}

/// The regression this issue's coordinator review actually found: a purge
/// must never re-notify a customer's webhook endpoint about a message they
/// were already told about — see the module doc's own "webhook-suppression
/// guard" section.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_purge_never_re_fires_a_webhook_to_a_registered_endpoint() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;
    let app_id = seed_app(&db).await;

    let old = Utc::now() - ChronoDuration::days(120);
    let delivered = drive_to_delivered(&db, &app_id, old).await;

    // Registered *after* the message already reached `delivered` — no
    // WebhookEndpoint existed at delivery time, so no WebhookAttempt row
    // exists yet for `webhook_attempts_dedupe` to have caught. This is
    // deliberately the one case dedupe cannot save: if the purge-site
    // guard is broken, this is where it shows.
    seed_webhook_endpoint(&db, &app_id, " message.delivered ").await;
    assert_eq!(
        count_attempts_for(&db, &delivered.id).await,
        0,
        "sanity: no WebhookAttempt should exist before the purge runs at all"
    );

    // A fresh Cratestack instance with subscribers registered — matching
    // production exactly: `app/sms-worker`'s `main` calls
    // `sms_api::webhooks::register_subscribers` unconditionally, regardless
    // of `--roles`, so the process actually running `purge_retention` in
    // production always has this wired up.
    let job_runner = fresh_db().await;
    sms_api::webhooks::register_subscribers(&job_runner);

    PurgeRetention
        .run_at(&job_runner, &sys(), Utc::now())
        .await
        .expect("purge_retention run_at succeeds");

    let purged = reload_message(&db, &delivered.id).await;
    assert!(
        purged.purgedAt.is_some(),
        "sanity: the message must actually have been purged for this test to mean anything"
    );

    assert_eq!(
        count_attempts_for(&db, &delivered.id).await,
        0,
        "a purge must never enqueue a WebhookAttempt for the message it just purged — \
         see webhooks.rs's own purgedAt guard"
    );
}
