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
use sms_provider::{
    Capabilities, DeliveryOutcome, DeliveryUpdate, Health, ProviderError, RawCallback, SmsProvider,
    SubmitAck, SubmitRequest,
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
///
/// This mutex serializes *execution* only. It does nothing about
/// *residual* state a previous test left behind in the same
/// never-reset-between-runs database — see [`clear_claimable_backlog`]'s
/// own doc for the second, independent isolation problem that turned out
/// to be, found live via an intermittently failing CI run.
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

/// Real test-isolation bug, found live via a ~30%-flaky CI run, not a
/// theoretical one: `dispatch`'s claim loop (`claim.rs::candidates()`)
/// selects *any* eligible `accepted`/`queued`/`routed` message system-wide
/// with an expired-or-absent lease — exactly matching production, since
/// the loop has no way to know which test seeded which row. This binary's
/// database (`sms-test-support`'s per-*binary* design, #118) is shared by
/// every test in this file and never reset between runs. `TEST_MUTEX`
/// prevents two tests from *executing* concurrently, but it does nothing
/// about *residual* state: a message a previous test (or a previous
/// session) left non-terminal is exactly as claimable as the row the
/// current test is about to seed, and `claim_batch`'s budget (up to
/// `tps_ceiling` rows per tick, §7.3) means a single `tick()` call can
/// claim *both* in the same batch — so a test's own `tick()` submits a
/// foreign leftover message to its own `wiremock` server on top of its
/// own, tripping that mock's `.expect(1)`. Confirmed as the actual
/// mechanism (not a race) by the failure shape: intermittent and
/// order-dependent, not constant, because it depends on what state a
/// *previous* test happened to leave behind.
///
/// Fixed by draining the claimable backlog to a terminal state before
/// every test seeds its own message, under the same `TEST_MUTEX` guard
/// every test already holds — so by induction, whatever backlog exists
/// when a test starts (from any earlier test, or an earlier session
/// entirely) is gone before that test's own row exists, and the test that
/// ran immediately before it cannot have left anything behind that
/// survives to the next one either.
///
/// `cancelled` is reachable directly from all three claimable states
/// (`accepted -> cancelled`, `queued -> cancelled`, `routed -> cancelled`
/// — §2.10), so one target state handles every row regardless of which of
/// the three it's currently sitting in; no need to branch on `state`
/// first. Through `CrateStack` delegates only (R1) — `if_match` per row,
/// same CAS discipline as every other writer of `Message`; a row that
/// moved on since it was listed (the trigger's own guard, or a genuinely
/// concurrent process) is logged and skipped, not fatal to this pass.
/// Loops in batches, since a backlog accumulated across many historical
/// runs before this fix existed can exceed one page.
async fn clear_claimable_backlog(db: &Cratestack) {
    const BATCH: usize = 500;
    let sys = sys();
    loop {
        let backlog = db
            .message()
            .find_many()
            .where_expr(cratestack::FilterExpr::from(schema::message::state().in_(
                [
                    MessageState::accepted,
                    MessageState::queued,
                    MessageState::routed,
                ],
            )))
            .limit(i64::try_from(BATCH).expect("BATCH fits in an i64"))
            .run(&sys)
            .await
            .expect("listing the claimable backlog");
        let drained = backlog.len();

        for message in backlog {
            let result = db
                .message()
                .update(message.id.clone())
                .set(schema::UpdateMessageInput {
                    state: Some(MessageState::cancelled),
                    ..Default::default()
                })
                .if_match(message.version)
                .run(&sys)
                .await;
            if let Err(error) = result {
                tracing::warn!(
                    message_id = %message.id,
                    %error,
                    "clearing the claimable test backlog: one row could not be cancelled"
                );
            }
        }

        if drained < BATCH {
            break;
        }
    }
    clear_undelivered_backlog(db).await;
}

/// #122: `undelivered` joined the claimable set (`Claimable for
/// Message::candidates()` now selects it for retry), so it needs the same
/// draining discipline as `accepted`/`queued`/`routed` above, for the same
/// cross-test-contamination reason `clear_claimable_backlog` itself exists
/// for. Separate loop, not folded into the one above: `cancelled` is
/// unreachable from `undelivered` (§2.10 has no such edge), so `failed` —
/// the legal terminal edge this state actually has — is the target here.
async fn clear_undelivered_backlog(db: &Cratestack) {
    const BATCH: usize = 500;
    let sys = sys();
    loop {
        let backlog = db
            .message()
            .find_many()
            .where_expr(cratestack::FilterExpr::from(
                schema::message::state().eq(MessageState::undelivered),
            ))
            .limit(i64::try_from(BATCH).expect("BATCH fits in an i64"))
            .run(&sys)
            .await
            .expect("listing the undelivered backlog");
        let drained = backlog.len();

        for message in backlog {
            let result = db
                .message()
                .update(message.id.clone())
                .set(schema::UpdateMessageInput {
                    state: Some(MessageState::failed),
                    ..Default::default()
                })
                .if_match(message.version)
                .run(&sys)
                .await;
            if let Err(error) = result {
                tracing::warn!(
                    message_id = %message.id,
                    %error,
                    "clearing the undelivered test backlog: one row could not be failed"
                );
            }
        }

        if drained < BATCH {
            break;
        }
    }
}

/// [`db`] plus [`clear_claimable_backlog`] — what every test in this file
/// should call instead of `db()` directly, so isolation can't be
/// forgotten at a new test's call site. See `clear_claimable_backlog`'s
/// own doc for why this matters.
async fn isolated_db() -> Cratestack {
    let db = db().await;
    clear_claimable_backlog(&db).await;
    db
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

/// Since #62, `claim.rs`'s routing pass evaluates every *enabled* `Route`
/// row against the whole database, not just whichever `Provider` this test
/// happened to create — and this database is never reset between runs, so
/// an earlier test's own catch-all route can still be sitting there,
/// enabled, pointing at a `Provider` this test's own [`WorkerContext`]
/// registry was never told about. If that stale route won the
/// priority/weight draw, `dispatch::resolve_provider` would fail to find
/// an adapter for it and back the message off instead of ever reaching
/// this test's wiremock server — a real flake, not a hypothetical one,
/// given `no_active_provider_rejects_before_any_submission_is_attempted`
/// and every ticket test run before it in the same binary all leave a
/// route behind. Disabling every existing route before seeding a fresh one
/// (mirroring [`deactivate_every_active_provider`]'s identical reasoning
/// for `Provider`) guarantees exactly one enabled route exists at claim
/// time: this test's own.
async fn disable_every_route(db: &Cratestack) {
    let enabled = db
        .route()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::route::enabled().is_true(),
        ))
        .run(&owner())
        .await
        .expect("listing enabled routes");
    for route in enabled {
        db.route()
            .update(route.id)
            .set(schema::UpdateRouteInput {
                enabled: Some(false),
                ..Default::default()
            })
            // #59: Route is @version'd now. This is a runtime requirement,
            // not a compile-time one — without it cratestack rejects the
            // write with `PreconditionFailed("If-Match header required for
            // versioned model")`, which `cargo check` cannot see.
            .if_match(route.version)
            .run(&owner())
            .await
            .expect("disabling a leftover enabled route");
    }
}

/// Seeds an active `Provider` plus a catch-all `Route` pointing at it
/// (after disabling every other enabled route — see
/// [`disable_every_route`]'s own doc), and returns the provider's `key` —
/// the exact string a caller must key its own `WorkerContext.providers`
/// registry with for `dispatch::resolve_provider` to find the adapter it
/// constructs against this test's wiremock server.
async fn seed_routed_provider(db: &Cratestack) -> String {
    disable_every_route(db).await;

    let key: String = format!("dispatch_test_{}", unique_suffix())
        .chars()
        .take(32)
        .collect();
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: key.clone(),
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
        // #59: Provider is now @version'd.
        .if_match(provider.version)
        .run(&owner())
        .await
        .expect("activating the provider");

    db.route()
        .create(schema::CreateRouteInput {
            name: format!("dispatch-test-route-{}", unique_suffix()),
            priority: 1000,
            weight: 1,
            enabled: true,
            matchOperator: None,
            matchClass: None,
            matchAppId: None,
            matchPrefix: None,
            providerId: provider.id,
            failoverRouteId: None,
        })
        .run(&owner())
        .await
        .expect("seeding a catch-all route");

    key
}

/// Builds the single-entry provider registry every test in this file needs
/// — see [`seed_routed_provider`]'s own doc for why the key must match
/// exactly.
fn registry(
    key: String,
    provider: Arc<dyn SmsProvider>,
) -> Arc<std::collections::HashMap<String, Arc<dyn SmsProvider>>> {
    Arc::new(std::collections::HashMap::from([(key, provider)]))
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
        let provider_version = provider.version;
        db.provider()
            .update(provider.id)
            .set(schema::UpdateProviderInput {
                state: Some(schema::ProviderState::disabled),
                ..Default::default()
            })
            // #59: Provider is now @version'd.
            .if_match(provider_version)
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
            msisdnHash: format!("hmac-sha256-v1:dispatch-test-{}", unique_suffix()),
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
            bodyHash: "hmac-sha256-v1:dispatch-test".to_owned(),
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
            purgedAt: None,
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

/// The production defaults (10s connect / 30s request) so every
/// already-fast, immediate-response test here is unaffected.
fn provider(base_url: String) -> Arc<OrangeCmProvider> {
    provider_with_timeouts(
        base_url,
        std::time::Duration::from_secs(10),
        std::time::Duration::from_secs(30),
    )
}

/// Same as [`provider`], but with caller-chosen timeouts — the
/// indeterminate-outcome test below needs a `request_timeout` short enough
/// that a deliberately delayed mock response actually fires it inside a
/// normal test run, rather than waiting on the 30s production default.
fn provider_with_timeouts(
    base_url: String,
    connect_timeout: std::time::Duration,
    request_timeout: std::time::Duration,
) -> Arc<OrangeCmProvider> {
    Arc::new(OrangeCmProvider::new(OrangeCmConfig {
        client_id: "client".to_owned(),
        client_secret: "secret".to_owned(),
        sender_number: "+2370000".to_owned(),
        base_url,
        dlr_notify_url: None,
        connect_timeout,
        request_timeout,
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
    let db = isolated_db().await;
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

    let provider_key = seed_routed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: registry(provider_key.clone(), provider(server.uri())),
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
    let db = isolated_db().await;
    let server = MockServer::start().await;
    mock_orange(&server).await;
    Mock::given(method("POST"))
        .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let provider_key = seed_routed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: registry(provider_key.clone(), provider(server.uri())),
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
    let db = isolated_db().await;
    let server = MockServer::start().await;
    mock_orange(&server).await;
    Mock::given(method("POST"))
        .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
        .respond_with(ResponseTemplate::new(400).set_body_string("destination unroutable"))
        .mount(&server)
        .await;

    let provider_key = seed_routed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: registry(provider_key.clone(), provider(server.uri())),
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
    let db = isolated_db().await;
    let server = MockServer::start().await;
    mock_orange(&server).await;
    Mock::given(method("POST"))
        .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1) // exactly one submit attempt — the second claim must not retry the provider
        .mount(&server)
        .await;

    let provider_key = seed_routed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 1).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: registry(provider_key.clone(), provider(server.uri())),
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
    let db = isolated_db().await;
    // Seeds this test's own route (disabling every stale one — see
    // `seed_routed_provider`'s own doc), then immediately deactivates
    // every active provider, this test's own included — guaranteeing at
    // least one enabled `Route` exists (so the routing engine actually has
    // something to evaluate and explain) while guaranteeing none of its
    // referenced providers are usable, deterministically, regardless of
    // what any earlier test in this binary left behind.
    let provider_key = seed_routed_provider(&db).await;
    deactivate_every_active_provider(&db).await;
    let server = MockServer::start().await;
    // No mocks registered at all — if dispatch ever tried to submit, the
    // request would fail with a connection error, not silently no-op.
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: registry(provider_key, provider(server.uri())),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds");

    let after = reload(&db, &seeded.id).await;
    assert_eq!(after.state, MessageState::rejected);
    assert!(
        after.stateReason.as_deref().is_some_and(
            |reason| reason.contains("no eligible route") && reason.contains("not active")
        ),
        "stateReason should explain why routing failed, got {:?}",
        after.stateReason
    );
}

/// A provider whose `parse_dlr` always returns exactly the updates it was
/// built with — every other method is unreachable from `sms_api::dlr::ingest`,
/// which only ever calls `parse_dlr`. Copied from
/// `crates/sms-api/tests/dlr_ingestion_live_postgres.rs`'s own
/// `FixedProvider`: that file can't be reused directly (a binary/lib test
/// boundary, same reason `op.rs`'s own routes get hand-rolled in more than
/// one test file elsewhere in this workspace), so this is the same small
/// shape re-declared here rather than a shared test-only crate for one
/// struct.
struct FixedProvider {
    updates: Vec<DeliveryUpdate>,
}

#[async_trait::async_trait]
impl SmsProvider for FixedProvider {
    fn key(&self) -> &'static str {
        "dispatch_test_fixed"
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

fn delivery_update_for(provider_ref: &str, outcome: DeliveryOutcome) -> DeliveryUpdate {
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

/// The core of this ticket, proven end to end against a real database and
/// a real (if deliberately delayed) HTTP round trip: a submit that times
/// out *after* Orange already had the request must land the message in
/// `uncertain`, and the claim loop must never touch it again — not "must
/// not resubmit within this test," but genuinely never, since `uncertain`
/// sits outside `claim::Claimable::candidates()`'s state filter.
///
/// The mock's `.expect(1)` on the submit route is the real assertion here,
/// not the message's final state alone: a implementation that got the
/// *state* right by luck (e.g. classified the timeout as `Permanent`
/// instead of routing through `Indeterminate`) but still let some other
/// path resubmit would pass a state-only check and fail this one.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_indeterminate_submit_lands_in_uncertain_and_is_never_resubmitted() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let server = MockServer::start().await;
    mock_orange(&server).await;
    Mock::given(method("POST"))
        .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
        // The delay exceeds the client's own request_timeout below, so
        // reqwest gives up and returns a genuine read timeout — the
        // connection was established (wiremock accepted it and is holding
        // the response), so this is squarely the "sent, no reply" case,
        // not a connect failure.
        .respond_with(ResponseTemplate::new(201).set_delay(std::time::Duration::from_millis(600)))
        .expect(1)
        .mount(&server)
        .await;

    let provider_key = seed_routed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: registry(
            provider_key.clone(),
            provider_with_timeouts(
                server.uri(),
                std::time::Duration::from_secs(2),
                std::time::Duration::from_millis(200),
            ),
        ),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // accepted -> queued
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued -> routed -> (timeout) -> uncertain

    let after = reload(&db, &seeded.id).await;
    assert_eq!(after.state, MessageState::uncertain);
    assert_eq!(
        after.providerMessageRefAlt,
        Some(seeded.id.clone()),
        "the reference sent as callbackData must be recorded even on a timed-out submit, or a \
         later DLR echoing it back can never correlate"
    );

    // Run several more ticks. `uncertain` is outside candidates()'s state
    // filter, so none of these should touch this message at all — and the
    // mock's own .expect(1), checked when the MockServer drops at the end
    // of this test, is what actually catches a resubmit if one happened.
    for _ in 0..3 {
        tick(&ctx, &sys, "worker-1").await.expect("tick succeeds");
    }
    let still_uncertain = reload(&db, &seeded.id).await;
    assert_eq!(still_uncertain.state, MessageState::uncertain);
    assert_eq!(
        still_uncertain.version, after.version,
        "no further write occurred at all"
    );
}

/// The other half of the ticket's own instruction — proving the new
/// variant wasn't over-applied: a failure that never got past the connect
/// phase must keep behaving exactly as before (back off and retry via
/// `queued`), not jump to `uncertain`.
///
/// This points the whole provider (token endpoint included) at an address
/// nothing listens on, so the connect refusal is actually surfaced by
/// `token::fetch`'s own hardcoded `Unavailable` mapping rather than
/// `classify_transport_error`'s connect branch specifically — the two
/// endpoints share one `base_url` and there is no way to fail only the
/// second from here. `classify_transport_error`'s connect-vs-timeout
/// predicate itself is unit-tested directly, against real reqwest errors,
/// in `sms-provider-orange-cm`'s own `a_connect_refusal_is_still_unavailable`
/// / `a_post_connect_timeout_is_indeterminate`. What this test proves is
/// the end-to-end dispatch behaviour: a connect-level failure must still
/// retry, never land in `uncertain`.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_connect_level_failure_still_backs_off_to_queued_not_uncertain() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
    let dead_addr = listener.local_addr().expect("reading the bound address");
    drop(listener);

    let provider_key = seed_routed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: registry(
            provider_key.clone(),
            provider_with_timeouts(
                format!("http://{dead_addr}"),
                std::time::Duration::from_millis(500),
                std::time::Duration::from_secs(2),
            ),
        ),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // accepted -> queued
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued -> routed -> (connect refused) -> queued

    let after = reload(&db, &seeded.id).await;
    assert_eq!(
        after.state,
        MessageState::queued,
        "a connect-level failure must still be safe to retry, not land in uncertain"
    );
    assert!(after
        .stateReason
        .is_some_and(|reason| reason.contains("provider unavailable")));
}

/// Closes the loop the design doc's own reasoning depends on: an
/// `uncertain` message is not abandoned, because
/// `providerMessageRefAlt` was recorded at timeout time (see the first
/// test above) and `sms_api::dlr::ingest_one` matches on
/// `providerMessageRef` **or** `providerMessageRefAlt`. A DLR arriving
/// later, echoing back the same reference Orange was sent as
/// `callbackData`, must still correlate and drive the message to a real
/// terminal state — proving the PR's own claim rather than asserting it
/// from documentation alone.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_dlr_after_an_indeterminate_submit_still_correlates_and_resolves() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let server = MockServer::start().await;
    mock_orange(&server).await;
    Mock::given(method("POST"))
        .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
        .respond_with(ResponseTemplate::new(201).set_delay(std::time::Duration::from_millis(600)))
        .mount(&server)
        .await;

    let provider_key = seed_routed_provider(&db).await;
    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: registry(
            provider_key.clone(),
            provider_with_timeouts(
                server.uri(),
                std::time::Duration::from_secs(2),
                std::time::Duration::from_millis(200),
            ),
        ),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // accepted -> queued
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued -> routed -> (timeout) -> uncertain

    let uncertain = reload(&db, &seeded.id).await;
    assert_eq!(uncertain.state, MessageState::uncertain);
    // Read off the message rather than assumed from `seed_routed_provider`'s
    // own return value: `seed_routed_provider` disables every other route
    // first (see its own doc), so this test's own route/provider pair is
    // deterministically the one that wins — but DLR correlation matches on
    // the *row id* the message was actually stamped with regardless, so
    // this reads it back from the message itself rather than leaning on
    // that determinism guarantee.
    let provider_row_id = uncertain
        .providerId
        .clone()
        .expect("routing must have stamped a providerId before this message could reach routed");

    let fixed_provider = FixedProvider {
        updates: vec![delivery_update_for(&seeded.id, DeliveryOutcome::Delivered)],
    };
    sms_api::dlr::ingest(
        &db,
        &sys,
        &fixed_provider,
        &provider_row_id,
        &empty_callback(),
    )
    .await
    .expect("ingest succeeds");

    let resolved = reload(&db, &seeded.id).await;
    assert_eq!(
        resolved.state,
        MessageState::delivered,
        "a late DLR echoing the callbackData reference must still resolve an uncertain message"
    );
}
