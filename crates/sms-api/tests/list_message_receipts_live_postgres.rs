//! `listMessageReceipts` (#50) against a real, fully migrated Postgres —
//! the actual `ProcedureRegistry` trait method, not a crate-private helper
//! called directly. Same discipline `requeue_job_live_postgres.rs` and
//! `replay_webhook_attempt_live_postgres.rs` both document in their own
//! module docs: **calling the trait method directly bypasses Layer 1
//! (`@allow`/`@authorize`) entirely** — that's enforced by the generated
//! router wrapping this method, not by the method itself. What *is*
//! exercised here: the Layer 2 `require_permission(ctx, "sms:read")` gate,
//! and the actual query this procedure runs — ordering, scoping to one
//! message, and the field projection (`rawPayload` never leaves this
//! layer). The `@authorize(Message, detail, args.messageId)` cross-app
//! denial this procedure relies on for real isolation is the identical gap
//! `requeue_job_live_postgres.rs`'s own trailing
//! `see_rbac_layer2_live_postgres_for_the_job_read_route_gate` stub names —
//! proving it needs a real HTTP round trip through the generated router,
//! which `rbac_layer2_live_postgres.rs`'s own suite is where this
//! codebase's route-level `@authorize`/`@allow` denials are proven live
//! (see e.g. its `PROVIDER_WRITE_ROUTES` case).
//!
//! ```bash
//! cargo test -p sms-api --test list_message_receipts_live_postgres -- --ignored
//! ```

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, CoolError, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, procedures::list_message_receipts, procedures::ProcedureRegistry, Cratestack, Encoding,
    MessageClass, MessageState, OperatorCode, UpdateMessageInput,
};
use sms_api::{HashPepper, Procedures};

/// #102: this binary's own tests can race on Postgres's own `pg_type`
/// catalog the first time two of them prepare the exact same not-yet-cached
/// query shape at the same instant — see `crates/sms-worker/tests/
/// claim_live_postgres.rs`'s own `TEST_MUTEX` doc for the full reasoning.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "list-message-receipts-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "list-message-receipts-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The context the admin console's own machine credential produces once
/// `SMS_CONSOLE_SCOPE` includes `sms:read` (`deploy/.env.example`, already
/// provisioned since #22/#24) — `kind == "app"`, matching `listMessageReceipts`'
/// broad `@allow`, plus the Layer 2 scope this procedure's own
/// `require_permission` checks.
fn app_caller_with_sms_read() -> CoolContext {
    let mut ctx = Principal {
        sub: "list-message-receipts-test-console-client".to_owned(),
        kind: PrincipalKind::App,
        role: "app".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions.insert(
        "scope".to_owned(),
        Value::String("sms:send sms:read".to_owned()),
    );
    ctx
}

/// The identical caller shape, but without `sms:read` — the exact
/// "an omitted scope yields denial" shape §5.2 documents.
fn app_caller_without_sms_read() -> CoolContext {
    let mut ctx = Principal {
        sub: "list-message-receipts-test-console-client-no-scope".to_owned(),
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
    HashPepper::new("list-message-receipts-live-postgres-test-pepper-over-the-minimum")
        .expect("test pepper meets HashPepper::new's minimum length")
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
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "list-message-receipts test app".to_owned(),
            slug: format!("lmr-test-{}", unique_suffix()),
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
            key: format!("lmr_test_{}", unique_suffix())
                .chars()
                .take(32)
                .collect(),
            displayName: "list-message-receipts test provider".to_owned(),
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
    provider.id
}

/// A freshly `accepted` message — `list_message_receipts` never depends on
/// the message's own state, only its id and `appId`, so no state-machine
/// walk is needed here (unlike `dlr_ingestion_live_postgres.rs`, which
/// needs a real `submitted` message to drive transitions from).
async fn seed_message(db: &Cratestack, app_id: &str) -> schema::Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("lmr-test-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:lmr-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 1000,
            body: Some("list-message-receipts test".to_owned()),
            bodyHash: "hmac-sha256-v1:lmr-test".to_owned(),
            bodyLength: 27,
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
            purgedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message")
}

async fn seed_receipt(
    db: &Cratestack,
    message_id: &str,
    provider_id: &str,
    outcome: schema::DeliveryOutcome,
    raw_status: &str,
) -> schema::DeliveryReceipt {
    db.delivery_receipt()
        .create(schema::CreateDeliveryReceiptInput {
            messageId: message_id.to_owned(),
            providerId: provider_id.to_owned(),
            providerMessageRef: format!("lmr-test-ref-{}", unique_suffix()),
            outcome,
            rawStatus: raw_status.to_owned(),
            errorCode: None,
            networkCode: OperatorCode::mtn,
            occurredAt: None,
            rawPayload: format!("{{\"raw\":\"{raw_status}\"}}"),
        })
        .run(&sys())
        .await
        .expect("seeding a delivery receipt")
}

/// The headline case: every receipt this message has, oldest `receivedAt`
/// first, with the console-facing projection — no `rawPayload` anywhere in
/// the result (the type this procedure returns has no field for it at
/// all, so this is really a compile-time guarantee, but the assertions
/// below also check the fields that ARE meant to survive).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn returns_every_receipt_for_the_message_oldest_first() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let provider_id = seed_provider(&db).await;
    let message = seed_message(&db, &app_id).await;

    let first = seed_receipt(
        &db,
        &message.id,
        &provider_id,
        schema::DeliveryOutcome::failed,
        "TemporaryError",
    )
    .await;
    // A DLR retry landing later — the chaos suite's own "more than one
    // receipt per message" shape (AGENTS.md's own framing of #34/#40).
    let second = seed_receipt(
        &db,
        &message.id,
        &provider_id,
        schema::DeliveryOutcome::delivered,
        "DeliveredToTerminal",
    )
    .await;
    assert!(
        first.receivedAt <= second.receivedAt,
        "the seed order must actually be the received order for this test to prove anything"
    );

    let result = Procedures::new(test_pepper())
        .list_message_receipts(
            &db,
            &app_caller_with_sms_read(),
            list_message_receipts::Args {
                args: schema::MessageReceiptsInput {
                    messageId: message.id.clone(),
                },
            },
        )
        .await
        .expect("listing receipts for an owned message must succeed");

    assert_eq!(
        result.receipts.len(),
        2,
        "both seeded receipts must come back"
    );
    assert_eq!(result.receipts[0].id, first.id, "oldest receivedAt first");
    assert_eq!(result.receipts[0].outcome, schema::DeliveryOutcome::failed);
    assert_eq!(result.receipts[0].rawStatus, "TemporaryError");
    assert_eq!(result.receipts[1].id, second.id);
    assert_eq!(
        result.receipts[1].outcome,
        schema::DeliveryOutcome::delivered
    );
}

/// A message with zero receipts is a completely normal outcome — the
/// `ProviderError::Indeterminate` case #50's own timeline design exists to
/// handle honestly. This procedure returns an empty list, not an error.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_message_with_no_receipts_returns_an_empty_list_not_an_error() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let message = seed_message(&db, &app_id).await;

    // Move it to `uncertain` the real way — walking every legal edge
    // (`accepted -> queued -> routed -> submitted -> uncertain`, §2.10),
    // never a shortcut straight to `submitted`, and never via
    // `Indeterminate` itself (that lives in sms-provider/sms-worker, out of
    // this crate's reach) — just to prove this procedure doesn't secretly
    // depend on the message being in any particular state.
    let queued = db
        .message()
        .update(message.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::queued),
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
            providerMessageRef: Some(Some(format!("lmr-uncertain-{}", unique_suffix()))),
            ..Default::default()
        })
        .if_match(routed.version)
        .run(&sys())
        .await
        .expect("routed -> submitted");
    let uncertain = db
        .message()
        .update(submitted.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::uncertain),
            ..Default::default()
        })
        .if_match(submitted.version)
        .run(&sys())
        .await
        .expect("submitted -> uncertain");
    assert_eq!(uncertain.state, MessageState::uncertain);

    let result = Procedures::new(test_pepper())
        .list_message_receipts(
            &db,
            &app_caller_with_sms_read(),
            list_message_receipts::Args {
                args: schema::MessageReceiptsInput {
                    messageId: uncertain.id.clone(),
                },
            },
        )
        .await
        .expect("an uncertain message with zero receipts must not error");

    assert!(
        result.receipts.is_empty(),
        "an Indeterminate-shaped message has no DeliveryReceipt at all — this must come back \
         empty, not synthesise one"
    );
}

/// Layer 2 (§5.1): an app-kind caller with no `sms:read` scope is denied
/// before the procedure touches the database at all — proven by pointing
/// it at a message id that doesn't even exist and confirming the error is
/// still `Forbidden`, not `NotFound` (which would mean the permission
/// check was skipped and the lookup ran anyway) — same proof shape
/// `requeue_job_live_postgres.rs`'s own
/// `requeue_denies_a_caller_with_no_job_enqueue_scope` uses.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn denies_a_caller_with_no_sms_read_scope() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let error = Procedures::new(test_pepper())
        .list_message_receipts(
            &db,
            &app_caller_without_sms_read(),
            list_message_receipts::Args {
                args: schema::MessageReceiptsInput {
                    messageId: "irrelevant-the-gate-must-fire-first".to_owned(),
                },
            },
        )
        .await
        .expect_err("a caller with no sms:read scope must be denied");

    assert!(
        matches!(error, CoolError::Forbidden(_)),
        "expected Forbidden, got {error:?}"
    );
    if let CoolError::Forbidden(message) = error {
        assert!(
            message.contains("sms:read"),
            "expected the denial to name the missing permission: {message}"
        );
    }
}

/// Break the guard, watch it fail with the exact denial symptom, then
/// leave it verified restored. Not a `#[test]` — this is the house-standard
/// "prove your guards can fail" exercise this PR's own description reports
/// the output of; kept here as a comment for anyone re-running the same
/// proof, matching `requeue_job_live_postgres.rs`'s own
/// `see_pr_description_for_the_guard_failure_proof` convention. The guard
/// actually broken and restored for this PR's own proof lives in
/// `admin/app/messages/[id]/timeline.test.ts` — the timeline honesty
/// guard, not this procedure's permission gate — see the PR description
/// for that exact terminal output.
#[allow(dead_code)]
fn see_pr_description_for_the_timeline_guard_failure_proof() {}
