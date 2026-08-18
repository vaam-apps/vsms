//! `dashboardSummary` (#49) against a real, fully migrated Postgres — the
//! actual `ProcedureRegistry` trait method, not `Procedures::
//! dashboard_snapshot` called directly, the same discipline
//! `rotate_webhook_secret_live_postgres.rs`/`worker_locks_live_postgres.rs`
//! document in their own module docs.
//!
//! The headline claim this file exists to prove, not just assert: **a
//! tile must not show a confidently wrong number.** `operator_stats_
//! excludes_uncertain_from_both_numerator_and_denominator` seeds one
//! `delivered` message and one `uncertain` message for the same operator
//! and asserts `terminalTotal == 1`, not `2` — folding `uncertain` into
//! the denominator (counting it as a failure) or into `delivered` (a lie)
//! are both wrong in a way that would still compile and still pass a test
//! that only checked "some numbers came back." This was verified to
//! actually fail, not just pass by construction: temporarily widening
//! `dashboard_snapshot`'s terminal-state filter in
//! `backends/crates/sms-api/src/procedures.rs` to include `MessageState::uncertain`
//! reproduced a real assertion failure (`terminalTotal: left: 2, right:
//! 1`) before the filter was restored — see this PR's own description for
//! the exact captured output.
//!
//! ```bash
//! cargo test -p sms-api --test dashboard_summary_live_postgres -- --ignored
//! ```

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, CratestackError, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
    procedures::ProcedureRegistry, procedures::dashboard_summary,
};
use sms_api::{HashPepper, Procedures};

/// Same reasoning as every other live suite's own copy of this mutex —
/// see `backends/crates/sms-worker/tests/claim_live_postgres.rs`'s doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> CratestackContext {
    Principal {
        sub: "dashboard-summary-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CratestackContext {
    Principal {
        sub: "dashboard-summary-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The console's own real shape: `kind == "app"`, scoped to one `appId`,
/// carrying the `dashboard:read` scope `require_permission` checks.
fn app_caller_with_dashboard_read(app_id: &str) -> CratestackContext {
    let mut ctx = Principal {
        sub: "dashboard-summary-test-console-client".to_owned(),
        kind: PrincipalKind::App,
        role: "app".to_owned(),
        app_id: app_id.to_owned(),
    }
    .into_context();
    ctx.extensions.insert(
        "scope".to_owned(),
        Value::String("sms:send dashboard:read".to_owned()),
    );
    ctx
}

fn app_caller_without_dashboard_read(app_id: &str) -> CratestackContext {
    let mut ctx = Principal {
        sub: "dashboard-summary-test-console-client-no-scope".to_owned(),
        kind: PrincipalKind::App,
        role: "app".to_owned(),
        app_id: app_id.to_owned(),
    }
    .into_context();
    ctx.extensions
        .insert("scope".to_owned(), Value::String("sms:send".to_owned()));
    ctx
}

fn test_pepper() -> HashPepper {
    HashPepper::new("dashboard-summary-live-postgres-test-pepper-well-over-the-minimum")
        .expect("test pepper meets HashPepper::new's minimum length")
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
            name: "dashboard summary test app".to_owned(),
            slug: format!("dashboard-summary-test-{}", unique_suffix()),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 100_000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the app")
        .id
}

async fn seed_message(
    db: &Cratestack,
    app_id: &str,
    operator: OperatorCode,
    encoding: Encoding,
) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: Some(format!("dashboard-summary-test-{}", unique_suffix())),
            idempotencyKey: Some(format!("dashboard-summary-test-{}", unique_suffix())),
            msisdn: "+237677000000".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:dashboard-summary-test-{}", unique_suffix()),
            operator,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 500,
            body: Some("dashboard summary test".to_owned()),
            bodyHash: format!("hmac-sha256-v1:dashboard-summary-test-{}", unique_suffix()),
            bodyLength: 24,
            encoding,
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
            expiresAt: Utc::now() + chrono::Duration::hours(1),
            submittedAt: None,
            finalizedAt: None,
            purgedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message")
}

/// Walks `message` through `path`, one legal edge at a time
/// (`message_state_transitions`, §2.10) — each hop a real `if_match`
/// write, the same CAS discipline every write in this codebase uses.
async fn drive_to(db: &Cratestack, message: Message, path: &[MessageState]) -> Message {
    let mut current = message;
    for &state in path {
        current = db
            .message()
            .update(current.id.clone())
            .set(schema::UpdateMessageInput {
                state: Some(state),
                ..Default::default()
            })
            .if_match(current.version)
            .run(&sys())
            .await
            .unwrap_or_else(|error| {
                panic!("driving message to {state:?} (from a legal edge): {error:?}")
            });
    }
    current
}

async fn seed_job(db: &Cratestack, kind: &str) -> schema::Job {
    db.job()
        .create(schema::CreateJobInput {
            kind: kind.to_owned(),
            dedupeKey: None,
            payload: "{}".to_owned(),
            priority: 500,
            runAt: Utc::now(),
            leaseOwner: None,
            leaseUntil: None,
            maxAttempts: 5,
            lastError: None,
            startedAt: None,
            finishedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the job")
}

async fn seed_endpoint(db: &Cratestack, app_id: &str) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(schema::CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: format!("https://example.test/webhooks/{}", unique_suffix()),
            eventTypes: " message.delivered ".to_owned(),
            secret: format!("test-secret-{}", unique_suffix()),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the endpoint")
}

async fn seed_webhook_attempt(db: &Cratestack, endpoint_id: &str) -> schema::WebhookAttempt {
    db.webhook_attempt()
        .create(schema::CreateWebhookAttemptInput {
            endpointId: endpoint_id.to_owned(),
            sourceEventId: cratestack::uuid::Uuid::new_v4(),
            aggregateId: format!("dashboard-summary-test-{}", unique_suffix()),
            eventType: "message.delivered".to_owned(),
            payload: "{}".to_owned(),
            leaseOwner: None,
            leaseUntil: None,
            nextAttemptAt: Some(Utc::now()),
            lastStatusCode: None,
            lastError: None,
            lastAttemptAt: None,
            deliveredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the webhook attempt")
}

fn find_operator(
    stats: &[schema::OperatorDeliveryStats],
    operator: OperatorCode,
) -> &schema::OperatorDeliveryStats {
    stats
        .iter()
        .find(|row| row.operator == operator)
        .unwrap_or_else(|| panic!("dashboardSummary did not report operator {operator:?} at all"))
}

/// A caller with no `dashboard:read` scope is refused before any query
/// runs — `DashboardSummary`'s own `@allow` admits any `auth().kind ==
/// "app"` caller unconditionally (it isn't a model), so this scope is the
/// real perimeter, not defense in depth.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_caller_with_no_dashboard_read_scope_is_denied() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — the "sanctioned way to invoke a procedure from
    // non-HTTP code" per that function's own doc comment.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_without_dashboard_read(&app_id);
    let args = dashboard_summary::Args {};
    let error = dashboard_summary::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.dashboard_summary(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect_err("a caller with no dashboard:read scope must be refused");

    assert!(
        matches!(error, CratestackError::Forbidden(_)),
        "expected Forbidden, got {error:?}"
    );
}

/// The headline case — see this file's own module doc for the failure
/// this guards against and the exact output captured breaking it on
/// purpose.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn operator_stats_excludes_uncertain_from_both_numerator_and_denominator() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;

    let delivered_msg = seed_message(&db, &app_id, OperatorCode::mtn, Encoding::gsm7).await;
    drive_to(
        &db,
        delivered_msg,
        &[
            MessageState::queued,
            MessageState::routed,
            MessageState::submitted,
            MessageState::delivered,
        ],
    )
    .await;

    let uncertain_msg = seed_message(&db, &app_id, OperatorCode::mtn, Encoding::gsm7).await;
    drive_to(
        &db,
        uncertain_msg,
        &[
            MessageState::queued,
            MessageState::routed,
            MessageState::uncertain,
        ],
    )
    .await;

    // cratestack 0.7.13 (cratestack#512): see the identical comment on the
    // test above.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_dashboard_read(&app_id);
    let args = dashboard_summary::Args {};
    let summary = dashboard_summary::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.dashboard_summary(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("dashboardSummary for a caller holding dashboard:read");

    let mtn = find_operator(&summary.operatorStats, OperatorCode::mtn);
    assert_eq!(mtn.delivered, 1, "exactly one message actually delivered");
    assert_eq!(
        mtn.terminalTotal, 1,
        "uncertain must not be counted as a terminal outcome — counting it \
         as failed overstates failure, counting it as delivered is a lie"
    );
    assert_eq!(
        summary.stuckMessages, 1,
        "the uncertain message must be reported on its own, not folded in"
    );
    assert_eq!(
        summary.queueDepth, 0,
        "both messages left the queue states (delivered/uncertain are not \
         accepted/queued/routed)"
    );
}

/// Throughput and the UCS-2 tile read off the same underlying scan
/// (`hourlyBuckets`) — this proves the split is correct, not just that
/// some total comes back.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn the_current_hour_bucket_splits_gsm7_and_ucs2_correctly() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;

    seed_message(&db, &app_id, OperatorCode::mtn, Encoding::gsm7).await;
    seed_message(&db, &app_id, OperatorCode::mtn, Encoding::gsm7).await;
    seed_message(&db, &app_id, OperatorCode::orange, Encoding::ucs2).await;

    // cratestack 0.7.13 (cratestack#512): see the identical comment above.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_dashboard_read(&app_id);
    let args = dashboard_summary::Args {};
    let summary = dashboard_summary::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.dashboard_summary(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("dashboardSummary for a caller holding dashboard:read");

    assert_eq!(
        summary.queueDepth, 3,
        "all three messages are still accepted"
    );

    let current_hour = summary
        .hourlyBuckets
        .last()
        .expect("hourlyBuckets must always report exactly six entries");
    assert_eq!(
        current_hour.totalCount, 3,
        "all three messages were created within the current hour"
    );
    assert_eq!(
        current_hour.ucs2Count, 1,
        "exactly one of the three was UCS-2"
    );
    assert_eq!(
        summary.hourlyBuckets.len(),
        6,
        "six rolling hours, always, even when most are empty"
    );
}

/// `Message` numbers are scoped to the caller's own `appId` — a second
/// app's console credential must see none of the first app's messages.
/// The console's own real-world shape (#211): `kind == "app"`, one fixed
/// `appId` per credential.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn message_based_tiles_are_scoped_to_the_callers_own_app() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_a = seed_app(&db).await;
    let app_b = seed_app(&db).await;

    seed_message(&db, &app_a, OperatorCode::mtn, Encoding::gsm7).await;
    seed_message(&db, &app_a, OperatorCode::mtn, Encoding::gsm7).await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`. Kept as two separate `Procedures::new(...)` calls,
    // matching the pre-existing shape, rather than sharing one instance.
    let ctx_b = app_caller_with_dashboard_read(&app_b);
    let args_b = dashboard_summary::Args {};
    let procedures_b = Procedures::new(test_pepper());
    let summary_b = dashboard_summary::invoke_with_db(&db, &args_b, &ctx_b, |authorized| {
        procedures_b.dashboard_summary(&db, &ctx_b, args_b.clone(), authorized)
    })
    .await
    .expect("dashboardSummary for app B's own caller");

    assert_eq!(
        summary_b.queueDepth, 0,
        "app B's own caller must not see app A's messages"
    );
    assert_eq!(summary_b.appId.as_deref(), Some(app_b.as_str()));

    let ctx_a = app_caller_with_dashboard_read(&app_a);
    let args_a = dashboard_summary::Args {};
    let procedures_a = Procedures::new(test_pepper());
    let summary_a = dashboard_summary::invoke_with_db(&db, &args_a, &ctx_a, |authorized| {
        procedures_a.dashboard_summary(&db, &ctx_a, args_a.clone(), authorized)
    })
    .await
    .expect("dashboardSummary for app A's own caller");
    assert_eq!(
        summary_a.queueDepth, 2,
        "app A's own caller sees both of its messages"
    );
}

/// `Job`/`WebhookAttempt` depth counts genuinely reflect the database —
/// checked as a before/after delta (a fresh `Procedures` per call, so
/// `dashboard_cache`'s 15s TTL never masks the seed) rather than an exact
/// value, since `Job` is system-wide and this binary's database is shared
/// across this file's own tests.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn job_backlog_and_outbox_depth_move_when_rows_are_seeded() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let endpoint = seed_endpoint(&db, &app_id).await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`. Kept as two separate `Procedures::new(...)` calls
    // (rather than one `procedures` shared across both `invoke_with_db`
    // calls) — this test's own doc comment says a fresh `Procedures` per
    // call is load-bearing so `dashboard_cache`'s 15s TTL never masks the
    // seed between "before" and "after".
    let ctx = app_caller_with_dashboard_read(&app_id);
    let args = dashboard_summary::Args {};
    let before_procedures = Procedures::new(test_pepper());
    let before = dashboard_summary::invoke_with_db(&db, &args, &ctx, |authorized| {
        before_procedures.dashboard_summary(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("dashboardSummary before seeding");

    seed_job(&db, "dashboard_summary_test_job").await;
    seed_webhook_attempt(&db, &endpoint.id).await;

    let after_procedures = Procedures::new(test_pepper());
    let after = dashboard_summary::invoke_with_db(&db, &args, &ctx, |authorized| {
        after_procedures.dashboard_summary(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("dashboardSummary after seeding");

    assert!(
        after.jobBacklog > before.jobBacklog,
        "jobBacklog must include the freshly seeded pending job: before={}, after={}",
        before.jobBacklog,
        after.jobBacklog
    );
    assert_eq!(
        after.outboxDepth,
        before.outboxDepth + 1,
        "outboxDepth is scoped to this app's own endpoint, so the delta must be exact"
    );
}
