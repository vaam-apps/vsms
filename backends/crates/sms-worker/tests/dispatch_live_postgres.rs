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
use cratestack::CratestackContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use sms_provider::{
    Capabilities, DeliveryOutcome, DeliveryUpdate, Health, ProviderError, RawCallback, SmsProvider,
    SubmitAck, SubmitRequest,
};
use sms_provider_orange_cm::{OrangeCmConfig, OrangeCmProvider};
use sms_worker::WorkerContext;
use sms_worker::dispatch::tick;
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

fn sys() -> CratestackContext {
    Principal {
        sub: "sms-worker-dispatch-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CratestackContext {
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
            circuitOpenUntil: None,
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
            excludedRouteIds: None,
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
    assert!(
        after
            .stateReason
            .is_some_and(|reason| reason.contains("400"))
    );
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
    assert!(
        after
            .stateReason
            .is_some_and(|reason| reason.contains("max attempts"))
    );
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
/// `backends/crates/sms-api/tests/dlr_ingestion_live_postgres.rs`'s own
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
    assert!(
        after
            .stateReason
            .is_some_and(|reason| reason.contains("provider unavailable"))
    );
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

// ---------------------------------------------------------------------
// #63: failover and the provider circuit breaker.
// ---------------------------------------------------------------------

/// A fake provider that always fails `submit` the same way and counts
/// exactly how many times it was called. `sms-provider-orange-cm`
/// structurally never produces `ProviderError::Permanent` — every 4xx it
/// sees other than 429 becomes `Rejected` (see that crate's own
/// `classify_submit_error` doc) — so the `TryNextRoute` failover arm can't
/// be exercised through the real adapter at all; this fake drives it
/// directly. The exact call count (not just the final `Message` state) is
/// what actually proves "never resubmitted"/"never even attempted" below —
/// a test that only checked state could pass even if something resubmitted
/// and got lucky.
struct AlwaysErr {
    key: String,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    error: fn() -> ProviderError,
}

impl AlwaysErr {
    fn new(
        key: impl Into<String>,
        error: fn() -> ProviderError,
    ) -> (Arc<Self>, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Arc::new(Self {
                key: key.into(),
                calls: calls.clone(),
                error,
            }),
            calls,
        )
    }
}

#[async_trait::async_trait]
impl SmsProvider for AlwaysErr {
    fn key(&self) -> &str {
        &self.key
    }
    fn capabilities(&self) -> Capabilities {
        // total_tps_ceiling reads this on every tick — unlike FixedProvider
        // above (a DLR-only fake, never resolved through the provider
        // registry `dispatch` sums budget from), this fake sits in
        // `WorkerContext.providers` and must return something real.
        Capabilities {
            dlr: true,
            alphanumeric_sender: true,
            ucs2: true,
            concatenation: true,
            tps_ceiling: 5.0,
            cost_per_segment_xaf: rust_decimal::Decimal::new(15, 0),
        }
    }
    async fn submit(&self, _req: &SubmitRequest) -> Result<SubmitAck, ProviderError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err((self.error)())
    }
    fn parse_dlr(&self, _raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
        unreachable!("dispatch never calls parse_dlr")
    }
    async fn health(&self) -> Health {
        unreachable!("dispatch never calls health")
    }
}

/// The failover target's fake — always succeeds, same call-counting
/// discipline as [`AlwaysErr`].
struct AlwaysOk {
    key: String,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl AlwaysOk {
    fn new(key: impl Into<String>) -> (Arc<Self>, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Arc::new(Self {
                key: key.into(),
                calls: calls.clone(),
            }),
            calls,
        )
    }
}

#[async_trait::async_trait]
impl SmsProvider for AlwaysOk {
    fn key(&self) -> &str {
        &self.key
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            dlr: true,
            alphanumeric_sender: true,
            ucs2: true,
            concatenation: true,
            tps_ceiling: 5.0,
            cost_per_segment_xaf: rust_decimal::Decimal::new(15, 0),
        }
    }
    async fn submit(&self, req: &SubmitRequest) -> Result<SubmitAck, ProviderError> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(SubmitAck {
            provider_ref: format!("scripted-ok-{n}-{}", req.reference),
            provider_ref_alt: None,
        })
    }
    fn parse_dlr(&self, _raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
        unreachable!("dispatch never calls parse_dlr")
    }
    async fn health(&self) -> Health {
        unreachable!("dispatch never calls health")
    }
}

/// Two active `Provider`s and two enabled catch-all `Route`s pointing at
/// them, after disabling every other enabled route (same isolation
/// reasoning as [`seed_routed_provider`]'s own doc) — `a` at priority 1000
/// (wins first, so it's the one that fails), `b` at priority 500 (the
/// failover target, so it only ever wins once `a` is excluded or its
/// circuit is open).
struct FailoverFixture {
    a_id: String,
    a_key: String,
    b_id: String,
    b_key: String,
}

/// One active `Provider`, labelled for [`seed_two_routed_providers`]'s own
/// two calls — pulled out to module scope (clippy's `items_after_statements`
/// would otherwise flag a nested `async fn` after the `disable_every_route`
/// call it would sit below).
async fn seed_one_active_provider(db: &Cratestack, label: &str) -> schema::Provider {
    let key: String = format!("dispatch_test_{label}_{}", unique_suffix())
        .chars()
        .take(32)
        .collect();
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key,
            displayName: format!("Dispatch test provider {label}"),
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
            circuitOpenUntil: None,
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
        .if_match(provider.version)
        .run(&owner())
        .await
        .expect("activating the provider");

    provider
}

async fn seed_two_routed_providers(db: &Cratestack) -> FailoverFixture {
    disable_every_route(db).await;

    let provider_a = seed_one_active_provider(db, "a").await;
    let provider_b = seed_one_active_provider(db, "b").await;

    db.route()
        .create(schema::CreateRouteInput {
            name: format!("dispatch-test-route-a-{}", unique_suffix()),
            priority: 1000,
            weight: 1,
            enabled: true,
            matchOperator: None,
            matchClass: None,
            matchAppId: None,
            matchPrefix: None,
            providerId: provider_a.id.clone(),
            failoverRouteId: None,
        })
        .run(&owner())
        .await
        .expect("seeding route a");

    db.route()
        .create(schema::CreateRouteInput {
            name: format!("dispatch-test-route-b-{}", unique_suffix()),
            priority: 500,
            weight: 1,
            enabled: true,
            matchOperator: None,
            matchClass: None,
            matchAppId: None,
            matchPrefix: None,
            providerId: provider_b.id.clone(),
            failoverRouteId: None,
        })
        .run(&owner())
        .await
        .expect("seeding route b");

    FailoverFixture {
        a_id: provider_a.id,
        a_key: provider_a.key,
        b_id: provider_b.id,
        b_key: provider_b.key,
    }
}

/// The guard `backends/crates/sms-worker/tests/dispatch_live_postgres.rs`'s own
/// (pre-existing, single-provider) `an_indeterminate_submit_lands_in_
/// uncertain_and_is_never_resubmitted` cannot actually prove: that test's
/// fixture has only one route, so a broken implementation that fails
/// `Indeterminate` over anyway would find nothing eligible to reroute to
/// and land in the identical `uncertain` outcome by accident — passing for
/// the wrong reason. This test uses the two-provider fixture specifically
/// so a real healthy alternative exists, making a wrongly-attempted
/// failover *visible*: if `handle_submit_error` ever treated
/// `RoutingConsequence::HoldIndeterminate` as failover-eligible, this
/// message would reroute to B, get claimed again, and reach `submitted`
/// there — a second, real HTTP call to a second provider for a message
/// that may already have been delivered by the first. `b_calls` staying at
/// `0` after several more ticks is the actual proof; the final `uncertain`
/// state alone would not be, for the same reason the single-provider test
/// can't distinguish the two cases.
///
/// Confirmed to actually catch this (house standard, `backends/crates/sms-provider`'s
/// own `ProviderError::routing()` test file follows the same discipline):
/// temporarily adding `RoutingConsequence::HoldIndeterminate` to
/// `handle_submit_error`'s `should_attempt_failover` match reproduced a
/// real failure here — `b_calls` read `1` and the message reached
/// `submitted` through the "healthy alternative" rather than staying
/// `uncertain` — before the line was reverted and this test re-confirmed
/// green.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_indeterminate_submit_never_fails_over_even_with_a_healthy_alternative_available() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let fixture = seed_two_routed_providers(&db).await;
    let (provider_a, a_calls) =
        AlwaysErr::new(fixture.a_key.clone(), || ProviderError::Indeterminate {
            message: "read timeout after the request was sent".to_owned(),
            source: None,
        });
    let (provider_b, b_calls) = AlwaysOk::new(fixture.b_key.clone());

    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: Arc::new(std::collections::HashMap::from([
            (fixture.a_key.clone(), provider_a as Arc<dyn SmsProvider>),
            (fixture.b_key.clone(), provider_b as Arc<dyn SmsProvider>),
        ])),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // accepted -> queued (routed to A)
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued -> routed -> (Indeterminate on A) -> uncertain

    let after = reload(&db, &seeded.id).await;
    assert_eq!(after.state, MessageState::uncertain);
    assert_eq!(
        after.providerMessageRefAlt,
        Some(seeded.id.clone()),
        "the reference sent before the network call must still be recorded"
    );

    // Several more ticks: `uncertain` is outside candidates()'s state
    // filter, so none of these should touch this message, and — the actual
    // assertion — B must never be attempted regardless.
    for _ in 0..3 {
        tick(&ctx, &sys, "worker-1").await.expect("tick succeeds");
    }
    let still_uncertain = reload(&db, &seeded.id).await;
    assert_eq!(still_uncertain.state, MessageState::uncertain);
    assert_eq!(
        a_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "A must be attempted exactly once"
    );
    assert_eq!(
        b_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "an Indeterminate outcome must never fail over, even when a healthy alternative (B) \
         exists — failing this over risks a duplicate SMS"
    );
}

/// #63's own worked example, end to end: a `Permanent` failure on the
/// winning route (`TryNextRoute`) must reroute — not fail outright, and
/// not resubmit through the same route/provider — to the next-best route,
/// and the message must still reach `submitted` there. Proves both halves
/// of the ticket's own mapping requirement at once: `Permanent` triggers
/// failover (unlike `Rejected`/`Unsupported`, `FailMessage`), and the
/// reroute never touches the provider's own circuit-breaker bookkeeping
/// (`permanent_never_opens_the_circuit_breaker`,
/// `backends/crates/sms-provider/src/error.rs`) — asserted directly against the
/// `Provider` row, not just inferred from the error taxonomy's own unit
/// test.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_permanent_failure_fails_over_to_the_next_route_and_reaches_submitted() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let fixture = seed_two_routed_providers(&db).await;
    let (provider_a, a_calls) =
        AlwaysErr::new(fixture.a_key.clone(), || ProviderError::Permanent {
            code: "SENDER_ID_NOT_APPROVED".to_owned(),
            message: "sender id not approved on this provider".to_owned(),
        });
    let (provider_b, b_calls) = AlwaysOk::new(fixture.b_key.clone());

    let app_id = seed_app(&db).await;
    let seeded = seed_message(&db, &app_id, 3).await;

    let ctx = WorkerContext {
        db: db.clone(),
        providers: Arc::new(std::collections::HashMap::from([
            (fixture.a_key.clone(), provider_a as Arc<dyn SmsProvider>),
            (fixture.b_key.clone(), provider_b as Arc<dyn SmsProvider>),
        ])),
    };
    let sys = sys();

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // accepted -> queued (routed to A, highest priority)
    let after_routing = reload(&db, &seeded.id).await;
    assert_eq!(after_routing.state, MessageState::queued);

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued -> routed -> (Permanent on A) -> failover -> queued (B)
    let after_failover = reload(&db, &seeded.id).await;
    assert_eq!(
        after_failover.state,
        MessageState::queued,
        "a failed-over message must be immediately reclaimable, not stuck"
    );
    assert_ne!(
        after_failover.providerId, after_routing.providerId,
        "failover must have stamped a different providerId"
    );
    assert!(
        after_failover
            .stateReason
            .as_deref()
            .is_some_and(|reason| reason.contains("failover")),
        "stateReason should explain the reroute, got {:?}",
        after_failover.stateReason
    );

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued -> routed -> submitted (via B)
    let after_submit = reload(&db, &seeded.id).await;
    assert_eq!(after_submit.state, MessageState::submitted);
    assert!(
        after_submit
            .providerMessageRef
            .as_deref()
            .is_some_and(|r| r.starts_with("scripted-ok-"))
    );

    assert_eq!(
        a_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the failing route must be attempted exactly once — the reroute must not retry it"
    );
    assert_eq!(
        b_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the failover route must be attempted exactly once — no double-send"
    );

    let provider_a_row = db
        .provider()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::provider::id().eq(fixture.a_id.clone()),
        ))
        .limit(1)
        .run(&sys)
        .await
        .expect("reloading provider a")
        .into_iter()
        .next()
        .expect("provider a still exists");
    assert_eq!(
        provider_a_row.consecutiveFailures, 0,
        "a Permanent failure must never touch the provider's own circuit-breaker bookkeeping"
    );
    assert!(provider_a_row.circuitOpenUntil.is_none());
}

/// The guard this ticket's own acceptance criterion names explicitly:
/// "must not fail a message a healthy alternative could carry." Five
/// separate messages each fail once against provider A (`Unavailable`,
/// which *does* record a circuit-breaker failure — unlike the `Permanent`
/// case above) and are individually failed over to B; the fifth failure
/// crosses §6.3's own five-consecutive-`Unavailable` threshold and opens
/// A's circuit. A sixth, brand-new message is then routed — proven not
/// merely to *succeed* via B after trying A, but to never attempt A at
/// all: `a_calls` stays at exactly 5 after the sixth message reaches
/// `submitted`, which is the strongest form of "a healthy alternative
/// carries it" this test can assert.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_open_circuit_routes_new_messages_to_the_alternative_instead_of_rejecting() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let fixture = seed_two_routed_providers(&db).await;
    let (provider_a, a_calls) =
        AlwaysErr::new(fixture.a_key.clone(), || ProviderError::Unavailable {
            message: "connection refused".to_owned(),
            source: None,
        });
    let (provider_b, b_calls) = AlwaysOk::new(fixture.b_key.clone());

    let ctx = WorkerContext {
        db: db.clone(),
        providers: Arc::new(std::collections::HashMap::from([
            (fixture.a_key.clone(), provider_a as Arc<dyn SmsProvider>),
            (fixture.b_key.clone(), provider_b as Arc<dyn SmsProvider>),
        ])),
    };
    let sys = sys();
    let app_id = seed_app(&db).await;

    // Five messages, each individually routed to A, failed over to B, and
    // resubmitted successfully there — driving A's own consecutiveFailures
    // from 0 to the five-failure threshold one message at a time.
    let mut seeded_ids = Vec::new();
    for _ in 0..5 {
        let seeded = seed_message(&db, &app_id, 3).await;
        seeded_ids.push(seeded.id);
    }

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // 5x accepted -> queued (all routed to A)
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // 5x queued -> routed -> (Unavailable on A) -> failover -> queued (B)
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // 5x queued -> routed -> submitted (via B)

    for id in &seeded_ids {
        let after = reload(&db, id).await;
        assert_eq!(
            after.state,
            MessageState::submitted,
            "every one of the five priming messages must still reach submitted via failover"
        );
    }
    assert_eq!(
        a_calls.load(std::sync::atomic::Ordering::SeqCst),
        5,
        "each priming message must attempt A exactly once before failing over"
    );
    assert_eq!(b_calls.load(std::sync::atomic::Ordering::SeqCst), 5);

    let provider_a_row = db
        .provider()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::provider::id().eq(fixture.a_id.clone()),
        ))
        .limit(1)
        .run(&sys)
        .await
        .expect("reloading provider a")
        .into_iter()
        .next()
        .expect("provider a still exists");
    assert!(
        provider_a_row
            .circuitOpenUntil
            .is_some_and(|until| until > Utc::now()),
        "five consecutive Unavailable failures must open the circuit breaker"
    );
    assert_eq!(
        provider_a_row.consecutiveFailures, 0,
        "opening the circuit resets the counter, matching hooks.rs's own reasoning"
    );

    // The guard itself: a sixth, brand-new message, routed fresh.
    let sixth = seed_message(&db, &app_id, 3).await;
    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // accepted -> queued
    let sixth_routed = reload(&db, &sixth.id).await;
    assert_eq!(sixth_routed.state, MessageState::queued);
    assert_eq!(
        sixth_routed.providerId,
        Some(fixture.b_id.clone()),
        "a fresh routing decision must skip A (open circuit) and land on B directly, not via a \
         per-message failover reroute"
    );

    tick(&ctx, &sys, "worker-1").await.expect("tick succeeds"); // queued -> routed -> submitted
    let sixth_final = reload(&db, &sixth.id).await;
    assert_eq!(sixth_final.state, MessageState::submitted);

    assert_eq!(
        a_calls.load(std::sync::atomic::Ordering::SeqCst),
        5,
        "provider A, whose circuit is open, must never even be attempted for a fresh message — \
         not just eventually failed over away from"
    );
    assert_eq!(
        b_calls.load(std::sync::atomic::Ordering::SeqCst),
        6,
        "the sixth message must have gone straight to the healthy alternative"
    );
}
