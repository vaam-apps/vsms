//! Proves `dispatch::tick` against a real, fully migrated Postgres and a
//! `wiremock`-backed `OrangeCmProvider` — the full `accepted -> queued ->
//! routed -> submitted` chain and `routed`'s reachable failure edges
//! (`ProviderError` variants `OrangeCmProvider` can actually produce;
//! `Permanent`/`Unsupported` aren't reachable through this adapter today —
//! see its own `classify_submit_error`).
//!
//! Ignored by default, same convention as this crate's other live suites.
//! Run explicitly:
//!
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-worker --test dispatch_live_postgres -- --ignored
//! ```

use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::CoolContext;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use sms_provider_orange_cm::{OrangeCmConfig, OrangeCmProvider};
use sms_worker::dispatch::tick;
use sms_worker::WorkerContext;
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// #102, found live: `dispatch::tick`'s own candidate query is
/// deliberately global (§7.3 — a real claim loop must see every app's
/// rows), so this binary's own tests, run concurrently by Rust's default
/// multi-threaded test harness, race on the same shared pool of
/// claimable messages. `no_active_provider_rejects_before_any_submission_is_attempted`
/// in particular calls `deactivate_every_active_provider`, which would
/// break every other concurrently-running test that depends on an active
/// provider existing. See `claim_live_postgres.rs`'s own `TEST_MUTEX` doc
/// for the full reasoning (including the first-use `pg_type` catalog
/// race this also fixes) — same mechanism, same fix.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "sms-worker-dispatch-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-worker-dispatch-test-owner".to_owned(),
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
            name: "dispatch test app".to_owned(),
            slug: format!("dispatch-test-{}", unique_suffix()),
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

async fn seed_active_provider(db: &Cratestack) -> String {
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!("dispatch_test_{}", unique_suffix())
                .chars()
                .take(32)
                .collect(),
            displayName: "Dispatch test provider".to_owned(),
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
        .run(&owner())
        .await
        .expect("activating the provider");

    provider.id
}

/// This database is never reset between runs, and both this file and
/// `claim_live_postgres.rs` leave `active` providers behind on purpose (so
/// a later `accepted` candidate has something to route to). A test that
/// specifically asserts "no active provider exists" needs a clean slate
/// regardless of what earlier runs left — deactivating every existing
/// active provider here, rather than trying to run this one test against
/// an empty database some other way.
async fn deactivate_every_active_provider(db: &Cratestack) {
    let active = db
        .provider()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::provider::state().eq(schema::ProviderState::active),
        ))
        .run(&owner())
        .await
        .expect("listing active providers");
    for provider in active {
        db.provider()
            .update(provider.id)
            .set(schema::UpdateProviderInput {
                state: Some(schema::ProviderState::disabled),
                ..Default::default()
            })
            .run(&owner())
            .await
            .expect("deactivating a leftover active provider");
    }
}

async fn seed_message(db: &Cratestack, app_id: &str, max_attempts: i64) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("dispatch-test-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("sha256:dispatch-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            // Max priority — see the identical comment in
            // claim_live_postgres.rs's own seed_message: this database is
            // never reset between runs, so a lower priority sorts behind
            // whatever earlier runs left in accepted/queued/routed and
            // silently drops out of a small budget.
            priority: 1000,
            body: Some("dispatch loop test".to_owned()),
            bodyHash: "sha256:dispatch-test".to_owned(),
            bodyLength: 19,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: None,
            providerId: None,
            providerMessageRef: None,
            providerMessageRefAlt: None,
            maxAttempts: max_attempts,
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

async fn mock_orange(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/oauth/v3/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "test-token",
            "expires_in": 3600,
        })))
        .mount(server)
        .await;
}

fn provider(base_url: String) -> Arc<OrangeCmProvider> {
    Arc::new(OrangeCmProvider::new(OrangeCmConfig {
        client_id: "client".to_owned(),
        client_secret: "secret".to_owned(),
        sender_number: "+2370000".to_owned(),
        base_url,
        dlr_notify_url: None,
    }))
}

async fn reload(db: &Cratestack, id: &str) -> Message {
    db.message()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::message::id().eq(id.to_owned()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("reloading the message")
        .into_iter()
        .next()
        .expect("the message still exists")
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_well_formed_message_reaches_submitted() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let server = MockServer::start().await;
    mock_orange(&server).await;
    Mock::given(method("POST"))
        .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "outboundSMSMessageRequest": {
                "resourceReference": {"resourceURL": "https://x/res-live-1"}
            }
        })))
        .mount(&server)
        .await;

    seed_active_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        provider: provider(server.uri()),
    };
    let sys = sys();

    // accepted -> queued
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds");
    let after_routing = reload(&db, &seeded.id).await;
    assert_eq!(after_routing.state, MessageState::queued);

    // queued -> routed -> submitted (submit succeeds within the same tick)
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds");
    let after_submit = reload(&db, &seeded.id).await;
    assert_eq!(after_submit.state, MessageState::submitted);
    assert_eq!(
        after_submit.providerMessageRef,
        Some("res-live-1".to_owned())
    );
    assert!(
        after_submit.submittedAt.is_some(),
        "submittedAt must be stamped by the trigger, not left unset"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_rate_limited_submit_backs_off_to_queued() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let server = MockServer::start().await;
    mock_orange(&server).await;
    Mock::given(method("POST"))
        .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    seed_active_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        provider: provider(server.uri()),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // accepted -> queued
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued -> routed -> (429) -> queued

    let after = reload(&db, &seeded.id).await;
    assert_eq!(after.state, MessageState::queued);
    assert_eq!(
        after.attempts, 1,
        "the failed submission still counts as one attempt"
    );
    assert!(
        after.leaseUntil.is_some_and(|until| until > Utc::now()),
        "a transient backoff must set a future leaseUntil, not leave the row immediately \
         reclaimable"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_rejected_submit_fails_the_message_outright() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let server = MockServer::start().await;
    mock_orange(&server).await;
    Mock::given(method("POST"))
        .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
        .respond_with(ResponseTemplate::new(400).set_body_string("destination unroutable"))
        .mount(&server)
        .await;

    seed_active_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        provider: provider(server.uri()),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds");
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds");

    let after = reload(&db, &seeded.id).await;
    assert_eq!(after.state, MessageState::failed);
    assert!(after
        .stateReason
        .is_some_and(|reason| reason.contains("400")));
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn exhausting_max_attempts_fails_the_message_without_a_further_submit_attempt() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let server = MockServer::start().await;
    mock_orange(&server).await;
    Mock::given(method("POST"))
        .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1) // exactly one submit attempt — the second claim must not retry the provider
        .mount(&server)
        .await;

    seed_active_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 1).await;

    let ctx = WorkerContext {
        db: db.clone(),
        provider: provider(server.uri()),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // accepted -> queued
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued -> routed -> (429) -> queued, attempts=1

    let after_first_attempt = reload(&db, &seeded.id).await;
    assert_eq!(after_first_attempt.state, MessageState::queued);
    assert_eq!(after_first_attempt.attempts, 1);

    // Force the backoff lease into the past — same technique
    // `claim_live_postgres.rs` uses to simulate time passing without a
    // real sleep.
    db.message()
        .update(after_first_attempt.id.clone())
        .set(schema::UpdateMessageInput {
            leaseUntil: Some(Some(Utc::now() - Duration::seconds(1))),
            ..Default::default()
        })
        .if_match(after_first_attempt.version)
        .run(&sys)
        .await
        .expect("forcing the backoff lease into the past");

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued, attempts>=maxAttempts -> failed

    let after = reload(&db, &seeded.id).await;
    assert_eq!(after.state, MessageState::failed);
    assert!(after
        .stateReason
        .is_some_and(|reason| reason.contains("max attempts")));
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn no_active_provider_rejects_before_any_submission_is_attempted() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    deactivate_every_active_provider(&db).await;
    let server = MockServer::start().await;
    // No mocks registered at all — if dispatch ever tried to submit, the
    // request would fail with a connection error, not silently no-op.
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        provider: provider(server.uri()),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds");

    let after = reload(&db, &seeded.id).await;
    assert_eq!(after.state, MessageState::rejected);
    assert!(after
        .stateReason
        .is_some_and(|reason| reason.contains("no active provider")));
}
