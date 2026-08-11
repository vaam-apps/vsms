//! Proves #38's `Message.created`/`Message.updated` subscribers
//! (`crates/sms-api/src/webhooks.rs`) against a real, fully migrated
//! Postgres: a matching `WebhookEndpoint` gets exactly one
//! `WebhookAttempt` row per catalogued event, a non-matching endpoint gets
//! none, an uncatalogued state transition produces no attempt at all, and
//! the `create` + catch-23505 dedupe path (§8.3) actually prevents a
//! duplicate row rather than just being written to.
//!
//! `a_created_message_produces_exactly_one_message_accepted_attempt`
//! covers the fix for a real bug caught in review: `message.accepted` is
//! documented (§8.4) and mapped by `message_event_type`, but is only ever
//! reachable from a `Message.created` event, never from `updated`
//! (`accepted` has no incoming edge in `message_state_transitions`) — see
//! that test's own doc for the full story.
//!
//! `a_message_transition_drains_through_the_real_registered_subscriber`
//! goes through `sms_api::webhooks::register_subscribers` and a real
//! `db.message().update(...)` call — not the lower-level
//! `enqueue_message_webhook_attempts` function directly — because the
//! property worth proving is the one `app/sms-gateway` and
//! `app/sms-worker` actually rely on: that a plain mutation on `Message`,
//! through the ordinary generated delegate, is enough to turn into a
//! `WebhookAttempt` row with no explicit `.events().drain()` call
//! anywhere in the test. See `webhooks.rs`'s own module doc for why that's
//! true (the framework's automatic post-commit drain) and why it depends
//! on subscribers being registered on *this* `Cratestack` instance first.
//!
//! Ignored by default, same convention as this crate's other live suites.
//! Run explicitly:
//!
//! ```bash
//! cargo test -p sms-api --test webhooks_live_postgres -- --ignored
//! ```

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, message, webhook_attempt, Cratestack, Encoding, Message, MessageClass, MessageState,
    OperatorCode,
};
use sms_api::webhooks::enqueue_message_webhook_attempts;

/// Same reasoning as every other live suite's own copy of this mutex —
/// see `crates/sms-worker/tests/claim_live_postgres.rs`'s doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "sms-api-webhooks-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-api-webhooks-test-owner".to_owned(),
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

async fn db() -> Cratestack {
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
            name: "webhooks test app".to_owned(),
            slug: format!("webhooks-test-{}", unique_suffix()),
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
    mask_recipient: bool,
) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(schema::CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: format!("https://example.test/webhooks/{}", unique_suffix()),
            eventTypes: event_types.to_owned(),
            secret: format!("test-secret-{}", unique_suffix()),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: mask_recipient,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding a WebhookEndpoint")
}

async fn seed_message(db: &Cratestack, app_id: &str, msisdn: &str) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: Some(format!("webhooks-test-{}", unique_suffix())),
            idempotencyKey: Some(format!("webhooks-test-{}", unique_suffix())),
            msisdn: msisdn.to_owned(),
            msisdnHash: format!("hmac-sha256-v1:webhooks-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 500,
            body: Some("webhooks subscriber test".to_owned()),
            bodyHash: format!("hmac-sha256-v1:webhooks-test-{}", unique_suffix()),
            bodyLength: 24,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: None,
            providerId: None,
            providerMessageRef: None,
            providerMessageRefAlt: None,
            maxAttempts: 3,
            leaseOwner: None,
            leaseUntil: None,
            scheduledAt: None,
            expiresAt: Utc::now() + Duration::hours(1),
            submittedAt: None,
            finalizedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message")
}

/// Reads back under `owner()`, not `sys()` — found live while writing this
/// suite: `WebhookAttempt`'s own `@@allow("list"/"detail", ...)` clause is
/// `auth().kind == "user" || endpoint.appId == auth().appId`, with no
/// `hasRole('system')` branch at all (unlike its `create`/`update`
/// clauses, which already have one). A `sys()` read here silently comes
/// back empty — this file's *own* would-be tenth instance of the repeated
/// gap `webhooks.rs`'s own module doc names eight of. Not fixed in this
/// PR: nothing #38/#39 actually builds ever reads `WebhookAttempt` under a
/// system context in production — only this test's own verification code
/// does — and the real production reader is #40's `hooks` claim loop,
/// which will need to add that clause itself when it lands. Flagged in
/// the PR description as a finding for whoever picks up #40.
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

/// The end-to-end path #38 and #39 both depend on: registering
/// subscribers on this test's own `Cratestack` instance, then performing
/// an ordinary `accepted -> cancelled` transition (a direct, one-hop
/// legal edge per `message_state_transitions`) through the generated
/// delegate — no explicit `.events().drain()` call anywhere in this test.
/// A matching endpoint gets exactly one `WebhookAttempt`; a non-matching
/// endpoint (subscribed to a different event type) gets none.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_cancelled_message_drains_through_the_real_registered_subscriber() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    sms_api::webhooks::register_subscribers(&db);

    let app_id = seed_app(&db).await;
    let matching = seed_endpoint(&db, &app_id, " message.cancelled ", true).await;
    let _non_matching = seed_endpoint(&db, &app_id, " message.delivered ", true).await;
    let message = seed_message(&db, &app_id, "+237677123456").await;

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

    let attempts = attempts_for(&db, &message.id).await;
    assert_eq!(
        attempts.len(),
        1,
        "expected exactly one WebhookAttempt for the matching endpoint, got {attempts:?}"
    );
    let attempt = &attempts[0];
    assert_eq!(attempt.endpointId, matching.id);
    assert_eq!(attempt.eventType, "message.cancelled");
    assert_eq!(attempt.aggregateId, message.id);
    assert_eq!(attempt.state, schema::AttemptState::pending);
    assert!(
        attempt.nextAttemptAt.is_some(),
        "nextAttemptAt must be set so webhook_due_idx finds this row"
    );

    // Masking: the seeded matching endpoint has maskRecipient = true, so
    // the stored payload must not contain the full plaintext msisdn.
    assert!(
        !attempt.payload.contains("677123456"),
        "payload should mask the recipient per the endpoint's own maskRecipient: {}",
        attempt.payload
    );
    assert!(
        attempt.payload.contains("cancelled"),
        "payload should carry the message's own post-update state: {}",
        attempt.payload
    );
}

/// **The eighth instance's own regression test.** `message.accepted` is
/// documented (§8.4) and `message_event_type` maps it — but `accepted` is
/// the schema's own `@default('accepted')` (the row's state the instant
/// it's created) and `message_state_transitions` lists it only as a
/// `from_state`, never a `to_state`: nothing transitions *into* `accepted`,
/// ever. Before this fix `register_subscribers` wired up only
/// `on_message_updated`, so `message.accepted` was advertised but
/// structurally unreachable — an endpoint subscribed to it would never
/// have fired, silently, forever. Found by Lightbridge's review of this
/// PR, confirmed against `message_state_transitions` before fixing.
///
/// Goes through a real `db.message().create(...)` (via [`seed_message`]),
/// not `enqueue_message_webhook_attempts` directly — the whole point is to
/// prove `on_message_created` actually fires through the ordinary create
/// path, the same way `a_cancelled_message_drains_through_the_real_registered_subscriber`
/// proves `on_message_updated` does. Then calls
/// `enqueue_message_webhook_attempts` a second time by hand, with a fresh
/// `event_id` but the same still-`accepted` message — standing in for
/// "drain retries this event, or a second worker races it" — and asserts
/// the row count stays at one: the `webhook_attempts_dedupe` unique index
/// (`endpoint_id`, `aggregate_id`, `event_type`) protects `message.accepted`
/// exactly the same way it protects every other catalogued event,
/// regardless of whether a `created` or an `updated` handler is what
/// produced the colliding insert.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_created_message_produces_exactly_one_message_accepted_attempt() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    sms_api::webhooks::register_subscribers(&db);

    let app_id = seed_app(&db).await;
    let matching = seed_endpoint(&db, &app_id, " message.accepted ", false).await;
    let _non_matching = seed_endpoint(&db, &app_id, " message.delivered ", false).await;

    // The create itself — no explicit .events().drain() call, no direct
    // call to enqueue_message_webhook_attempts — is what must produce the
    // attempt, via on_message_created's own automatic post-commit drain.
    let message = seed_message(&db, &app_id, "+237677123461").await;

    let attempts = attempts_for(&db, &message.id).await;
    assert_eq!(
        attempts.len(),
        1,
        "Message.created should have produced exactly one message.accepted \
         WebhookAttempt via on_message_created: {attempts:?}"
    );
    assert_eq!(attempts[0].endpointId, matching.id);
    assert_eq!(attempts[0].eventType, "message.accepted");
    assert_eq!(attempts[0].aggregateId, message.id);

    // Simulates a retry/race on the same (endpoint, aggregate, event_type)
    // — must not duplicate.
    enqueue_message_webhook_attempts(&db, cratestack::uuid::Uuid::new_v4(), &message)
        .await
        .expect("a second enqueue for the same event must swallow the 23505, not error");

    let attempts_after = attempts_for(&db, &message.id).await;
    assert_eq!(
        attempts_after.len(),
        1,
        "webhook_attempts_dedupe should still block a second message.accepted attempt \
         for the same message: {attempts_after:?}"
    );
}

/// `accepted -> queued` is a legal one-hop edge, but `queued` isn't in
/// §8.4's event catalogue (internal routing machinery) — no
/// `WebhookAttempt` row should be created for it at all, matching
/// edge-for-edge, not "every transition produces a webhook."
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_uncatalogued_state_transition_produces_no_webhook_attempt() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    sms_api::webhooks::register_subscribers(&db);

    let app_id = seed_app(&db).await;
    // Deliberately not subscribed to "message.accepted" — that event is
    // real now (see a_created_message_produces_exactly_one_message_accepted_attempt)
    // and would otherwise produce one legitimate attempt from
    // seed_message's own create() call, muddying this test's own "queued
    // produces nothing" assertion. This endpoint only cares about
    // "delivered", so both the create (accepted, unmatched) and the
    // update below (queued, uncatalogued) should leave it with zero.
    seed_endpoint(&db, &app_id, " message.delivered ", false).await;
    let message = seed_message(&db, &app_id, "+237677123457").await;

    db.message()
        .update(message.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(MessageState::queued),
            ..Default::default()
        })
        .if_match(message.version)
        .run(&sys())
        .await
        .expect("moving the message to queued");

    let attempts = attempts_for(&db, &message.id).await;
    assert!(
        attempts.is_empty(),
        "queued isn't in §8.4's catalogue and this endpoint never subscribed to \
         message.accepted either; expected no attempts, got {attempts:?}"
    );
}

/// §8.3's own dedupe reasoning, proven directly against
/// `enqueue_message_webhook_attempts` rather than through the outbox: two
/// calls describing the same (endpoint, aggregate, `event_type`) — as would
/// happen if two drains raced, or if the same message is updated twice
/// in a way that both derive the same event type — produce exactly one
/// `WebhookAttempt` row, not two. The second call's `23505` on
/// `webhook_attempts_dedupe` must be swallowed, not propagated.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn duplicate_enqueue_of_the_same_event_is_deduped() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let app_id = seed_app(&db).await;
    seed_endpoint(&db, &app_id, " message.accepted ", false).await;
    let message = seed_message(&db, &app_id, "+237677123458").await;

    let event_id_a = cratestack::uuid::Uuid::new_v4();
    let event_id_b = cratestack::uuid::Uuid::new_v4();

    enqueue_message_webhook_attempts(&db, event_id_a, &message)
        .await
        .expect("first enqueue");
    enqueue_message_webhook_attempts(&db, event_id_b, &message)
        .await
        .expect("second enqueue must swallow the 23505, not error");

    let attempts = attempts_for(&db, &message.id).await;
    assert_eq!(
        attempts.len(),
        1,
        "webhook_attempts_dedupe should have blocked the second insert: {attempts:?}"
    );
    // The dedupe key is (endpoint_id, aggregate_id, event_type), not
    // source_event_id — §8.3's own reasoning for why. The surviving row
    // keeps whichever event_id won the race (the first one, here).
    assert_eq!(attempts[0].sourceEventId, event_id_a);
}

/// A message with no `WebhookEndpoint` at all for its app is the common
/// case (most apps in this milestone's demo never configure one) — must
/// not error, must simply produce nothing.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_message_with_no_matching_endpoints_produces_no_attempts_and_no_error() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let app_id = seed_app(&db).await;
    let message = seed_message(&db, &app_id, "+237677123459").await;

    enqueue_message_webhook_attempts(&db, cratestack::uuid::Uuid::new_v4(), &message)
        .await
        .expect("no endpoints configured should not be an error");

    let attempts = attempts_for(&db, &message.id).await;
    assert!(attempts.is_empty());
}

/// **The one honest caveat under `#44`'s "no event lost" gate.** Read
/// `webhooks.rs`'s own module doc before this test: on the Postgres
/// backend, `CoolEventBus::emit` returns `Ok(())` for a topic with zero
/// registered handlers, and the framework's own automatic post-commit
/// drain treats `Ok` the same whether a handler actually ran or there
/// never was one to run — it marks the outbox row `delivered_at = NOW()`
/// either way. So a `Cratestack` instance that never calls
/// `register_subscribers` doesn't leave its own writes for a later,
/// correctly-registered drain to "catch up" on later: the row is already
/// marked delivered, having done nothing, the moment the write commits.
/// This test makes that a permanent, reproducible assertion rather than
/// prose. It is not a bug this PR fixes (there is nothing in this crate to
/// fix — the behaviour lives in `cratestack-sqlx`'s own `drain_event_outbox`,
/// read directly in `crates/sms-worker/src/jobs/reap_outbox.rs`'s own module
/// doc) and it is why `AGENTS.md`'s M3 section calls registration
/// "mandatory plumbing in every process," not optional wiring: every real
/// writer in this codebase (`app/sms-gateway`'s `serve`, `app/sms-worker`'s
/// `main`) calls `register_subscribers` unconditionally before touching
/// `db`, which is what keeps this exact scenario from ever happening in
/// production. `#44`'s own "kill sms-api mid-drain, no event is lost" gate
/// is proven separately, against a real spawned `sms-gateway serve` process
/// that (like every real deployment) does call `register_subscribers` —
/// see `app/sms-gateway/tests/webhook_outbox_kill_mid_drain_live.rs`. This
/// test exists to draw the boundary of that guarantee explicitly: it holds
/// because registration is unconditional in every real writer, not because
/// the outbox is magically loss-proof regardless of whether anything is
/// listening.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_writer_that_never_registered_subscribers_silently_loses_the_event() {
    let _guard = TEST_MUTEX.lock().await;
    let unregistered_db = db().await;

    let app_id = seed_app(&unregistered_db).await;
    seed_endpoint(&unregistered_db, &app_id, " message.cancelled ", true).await;
    let message = seed_message(&unregistered_db, &app_id, "+237677123463").await;

    // No `register_subscribers` call on `unregistered_db` at all. The
    // transition below still commits successfully — R2/the trigger don't
    // care whether anyone is listening — and its own automatic post-commit
    // drain still runs (`create.rs`/`update.rs`'s unconditional call), but
    // `CoolEventBus::emit` has zero handlers for this topic on *this*
    // instance, so it's `Ok(())` immediately and the row is marked
    // delivered having done nothing.
    unregistered_db
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

    let attempts = attempts_for(&unregistered_db, &message.id).await;
    assert!(
        attempts.is_empty(),
        "an unregistered writer's own automatic drain marks the outbox row delivered having \
         done nothing — this is the documented gap this test exists to pin down, not something \
         it expects to be fixed: {attempts:?}"
    );

    // The sharper half: a *second*, properly registered instance draining
    // the same table afterwards does not recover this event either,
    // because the row already reads delivered_at IS NOT NULL — there is no
    // "eventually consistent" backstop for this specific failure mode,
    // unlike a genuinely-still-undelivered row (which `drain_live_postgres.rs`
    // and `#44`'s own kill-mid-drain gate both prove recovers correctly).
    let registered_db = db().await;
    sms_api::webhooks::register_subscribers(&registered_db);
    registered_db
        .events()
        .drain()
        .await
        .expect("draining the registered instance's own runtime");

    let attempts_after_drain = attempts_for(&registered_db, &message.id).await;
    assert!(
        attempts_after_drain.is_empty(),
        "confirms the event is permanently lost, not merely delayed: a later, correctly \
         registered drain has nothing left to redeliver, since delivered_at was already set \
         true by the unregistered writer: {attempts_after_drain:?}"
    );
}

/// A fixture sanity check, not a subscriber assertion: every test above
/// relies on `seed_message` producing a row in `accepted` so that
/// `accepted -> cancelled` and `accepted -> queued` are the direct,
/// one-hop legal edges (`message_state_transitions`) the tests use to
/// avoid walking the full `accepted -> queued -> routed -> submitted ->
/// delivered` chain just to exercise the subscriber.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn seeded_messages_start_accepted() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let message = seed_message(&db, &app_id, "+237677123460").await;

    let found = db
        .message()
        .find_many()
        .where_expr(FilterExpr::from(message::id().eq(message.id.clone())))
        .run(&sys())
        .await
        .expect("reading the message back");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].state, MessageState::accepted);
}
