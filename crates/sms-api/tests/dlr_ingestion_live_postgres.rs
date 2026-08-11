//! Proves `dlr::ingest` against a real, fully migrated Postgres — matching
//! a `DeliveryUpdate` to a `Message` by `providerMessageRef`, writing the
//! `DeliveryReceipt`, and driving the state machine, including the one
//! subtle correctness detail `next_state` exists for: a retryable failure
//! arriving while a message is `uncertain` must land it in `failed`, not
//! `undelivered` — `uncertain -> undelivered` isn't a legal edge (§2.10).
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-api --test dlr_ingestion_live_postgres -- --ignored
//! ```

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, message, Cratestack, Encoding, MessageClass, MessageState, OperatorCode,
    UpdateMessageInput,
};
use sms_provider::{
    Capabilities, DeliveryOutcome, DeliveryUpdate, Health, ProviderError, RawCallback, SmsProvider,
    SubmitAck, SubmitRequest,
};

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests — run concurrently by Rust's default multi-threaded test
/// harness — can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape (e.g.
/// `db.provider().create(...)`) at the same instant: `duplicate key
/// value violates unique constraint "pg_type_typname_nsp_index"`. See
/// `crates/sms-worker/tests/claim_live_postgres.rs`'s own `TEST_MUTEX`
/// doc for the full reasoning — same mechanism, same fix, applied here
/// even though this file has no candidate-query contamination risk of
/// its own (every test already scopes its own lookups to its own seeded
/// message).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "dlr-ingestion-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "dlr-ingestion-test-owner".to_owned(),
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
        .expect("system clock is after the epoch")
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
            name: "dlr ingestion test app".to_owned(),
            slug: format!("dlr-test-{}", unique_suffix()),
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

async fn seed_provider(db: &Cratestack) -> String {
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!("dlr_test_{}", unique_suffix())
                .chars()
                .take(32)
                .collect(),
            displayName: "DLR test provider".to_owned(),
            kind: schema::ProviderKind::orange_cm_http,
            config: "{}".to_owned(),
            credentialRef: "vault://test".to_owned(),
            maxTps: 5.0,
            maxDailySubmissions: 1000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: "15".parse().unwrap(),
            healthCheckedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding a provider");

    db.provider()
        .update(provider.id.clone())
        .set(schema::UpdateProviderInput {
            state: Some(schema::ProviderState::active),
            ..Default::default()
        })
        // #59: Provider is now @version'd.
        .if_match(provider.version)
        .run(&owner())
        .await
        .expect("activating the provider");

    provider.id
}

/// Walks a fresh message through the *real* legal chain — `create` can
/// only ever produce `accepted` (`@default('accepted')`) — up to
/// `submitted`, with `providerId`/`providerMessageRef` stamped along the
/// way, matching what `dispatch`'s own claim loop does. No shortcuts
/// around R2: each hop is a real, trigger-checked transition.
async fn seed_submitted_message(
    db: &Cratestack,
    app_id: &str,
    provider_id: &str,
) -> schema::Message {
    let provider_ref = format!("dlr-test-ref-{}", unique_suffix());
    let created = db
        .message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("dlr-test-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:dlr-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 1000,
            body: Some("dlr ingestion test".to_owned()),
            bodyHash: "hmac-sha256-v1:dlr-test".to_owned(),
            bodyLength: 19,
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
        .expect("seeding the message");

    let queued = db
        .message()
        .update(created.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::queued),
            providerId: Some(Some(provider_id.to_owned())),
            ..Default::default()
        })
        .if_match(created.version)
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

    db.message()
        .update(routed.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::submitted),
            providerMessageRef: Some(Some(provider_ref)),
            ..Default::default()
        })
        .if_match(routed.version)
        .run(&sys())
        .await
        .expect("routed -> submitted")
}

async fn reload(db: &Cratestack, id: &str) -> schema::Message {
    db.message()
        .find_many()
        .where_expr(FilterExpr::from(message::id().eq(id.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("reloading the message")
        .into_iter()
        .next()
        .expect("the message still exists")
}

async fn receipts_for(db: &Cratestack, message_id: &str) -> Vec<schema::DeliveryReceipt> {
    db.delivery_receipt()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::delivery_receipt::messageId().eq(message_id.to_owned()),
        ))
        .run(&owner())
        .await
        .expect("listing receipts")
}

/// A provider whose `parse_dlr` always returns exactly the updates it was
/// built with — every other method is unreachable from `dlr::ingest`,
/// which only ever calls `parse_dlr`.
struct FixedProvider {
    updates: Vec<DeliveryUpdate>,
}

#[async_trait::async_trait]
impl SmsProvider for FixedProvider {
    fn key(&self) -> &'static str {
        "dlr_test"
    }
    fn capabilities(&self) -> Capabilities {
        unreachable!("dlr::ingest never calls capabilities")
    }
    async fn submit(&self, _req: &SubmitRequest) -> Result<SubmitAck, ProviderError> {
        unreachable!("dlr::ingest never calls submit")
    }
    fn parse_dlr(&self, _raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
        Ok(self.updates.clone())
    }
    async fn health(&self) -> Health {
        unreachable!("dlr::ingest never calls health")
    }
}

fn update_for(provider_ref: &str, outcome: DeliveryOutcome) -> DeliveryUpdate {
    DeliveryUpdate {
        provider_ref: provider_ref.to_owned(),
        outcome,
        occurred_at: None,
        raw_status: format!("{outcome:?}"),
        error_code: None,
        delivering_network: None,
    }
}

fn empty_callback() -> RawCallback {
    RawCallback {
        headers: vec![],
        body: b"{}".to_vec(),
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_delivered_dlr_transitions_a_submitted_message_and_writes_a_receipt() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let provider_id = seed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let message = seed_submitted_message(&db, &app_id, &provider_id).await;
    let provider_ref = message.providerMessageRef.clone().unwrap();

    let provider = FixedProvider {
        updates: vec![update_for(&provider_ref, DeliveryOutcome::Delivered)],
    };
    sms_api::dlr::ingest(&db, &sys(), &provider, &provider_id, &empty_callback())
        .await
        .expect("ingest succeeds");

    let after = reload(&db, &message.id).await;
    assert_eq!(after.state, MessageState::delivered);

    let receipts = receipts_for(&db, &message.id).await;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, schema::DeliveryOutcome::delivered);
    assert_eq!(receipts[0].providerMessageRef, provider_ref);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_unmatched_provider_ref_is_ignored_not_an_error() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let provider_id = seed_provider(&db).await;

    let provider = FixedProvider {
        updates: vec![update_for(
            &format!("no-such-ref-{}", unique_suffix()),
            DeliveryOutcome::Delivered,
        )],
    };
    // The real assertion is "this does not return Err" — a DLR referencing
    // a message this deployment never issued (a stale webhook, a purged
    // row) must not fail the callback.
    sms_api::dlr::ingest(&db, &sys(), &provider, &provider_id, &empty_callback())
        .await
        .expect("an unmatched ref must not error the whole callback");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_retryable_failure_from_submitted_goes_to_undelivered() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let provider_id = seed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let message = seed_submitted_message(&db, &app_id, &provider_id).await;
    let provider_ref = message.providerMessageRef.clone().unwrap();

    let provider = FixedProvider {
        updates: vec![update_for(&provider_ref, DeliveryOutcome::Failed)],
    };
    sms_api::dlr::ingest(&db, &sys(), &provider, &provider_id, &empty_callback())
        .await
        .expect("ingest succeeds");

    let after = reload(&db, &message.id).await;
    assert_eq!(after.state, MessageState::undelivered);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_retryable_failure_from_uncertain_goes_to_failed_not_undelivered() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let provider_id = seed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let submitted = seed_submitted_message(&db, &app_id, &provider_id).await;
    let provider_ref = submitted.providerMessageRef.clone().unwrap();

    // submitted -> uncertain, via a first DLR.
    let uncertain_provider = FixedProvider {
        updates: vec![update_for(&provider_ref, DeliveryOutcome::Uncertain)],
    };
    sms_api::dlr::ingest(
        &db,
        &sys(),
        &uncertain_provider,
        &provider_id,
        &empty_callback(),
    )
    .await
    .expect("ingest succeeds");
    let uncertain = reload(&db, &submitted.id).await;
    assert_eq!(uncertain.state, MessageState::uncertain);

    // A second, later DLR reports a retryable failure. §2.10 has no
    // `uncertain -> undelivered` edge — this must land in `failed`.
    let failure_provider = FixedProvider {
        updates: vec![update_for(&provider_ref, DeliveryOutcome::Failed)],
    };
    sms_api::dlr::ingest(
        &db,
        &sys(),
        &failure_provider,
        &provider_id,
        &empty_callback(),
    )
    .await
    .expect("ingest succeeds");

    let after = reload(&db, &submitted.id).await;
    assert_eq!(after.state, MessageState::failed);

    // Both DLRs are still recorded, even though only the second changed
    // the message's own state.
    let receipts = receipts_for(&db, &submitted.id).await;
    assert_eq!(receipts.len(), 2);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_stale_dlr_after_the_message_already_finalised_is_swallowed_not_an_error() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let provider_id = seed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let message = seed_submitted_message(&db, &app_id, &provider_id).await;
    let provider_ref = message.providerMessageRef.clone().unwrap();

    // First DLR: delivered — a real, correct terminal transition.
    let delivered_provider = FixedProvider {
        updates: vec![update_for(&provider_ref, DeliveryOutcome::Delivered)],
    };
    sms_api::dlr::ingest(
        &db,
        &sys(),
        &delivered_provider,
        &provider_id,
        &empty_callback(),
    )
    .await
    .expect("ingest succeeds");
    assert_eq!(
        reload(&db, &message.id).await.state,
        MessageState::delivered
    );

    // A second, late/duplicate DLR for the same ref arrives after the
    // message is already terminal. `delivered -> uncertain` isn't a legal
    // edge — this must not fail the callback, just leave the message
    // exactly where it is.
    let late_provider = FixedProvider {
        updates: vec![update_for(&provider_ref, DeliveryOutcome::Uncertain)],
    };
    sms_api::dlr::ingest(&db, &sys(), &late_provider, &provider_id, &empty_callback())
        .await
        .expect("a stale DLR must not error the callback");

    let after = reload(&db, &message.id).await;
    assert_eq!(
        after.state,
        MessageState::delivered,
        "a stale DLR must not move a message off its already-final state"
    );

    // The stale DLR still gets a receipt written — nothing about it is
    // lost, it just didn't move the message.
    let receipts = receipts_for(&db, &message.id).await;
    assert_eq!(receipts.len(), 2);
}
