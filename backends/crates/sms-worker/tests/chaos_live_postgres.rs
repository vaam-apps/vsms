//! Fault-injecting chaos suite for the message state machine — the
//! automatable complement to `docs/runbooks/36-handset-gate.adoc`, **not** a
//! replacement for it. `sms-fake-orange` (see its own module doc) is a
//! participant in the send/DLR lifecycle: it answers submit HTTP calls per
//! a fault policy, autonomously schedules DLR deliveries against a real,
//! in-process `POST /dlr/{providerKey}` route, and keeps a request ledger.
//!
//! # What this suite can and cannot prove
//!
//! It proves this system's own state machine holds its invariants under
//! the failure modes `sms-provider-orange-cm`'s own error classification
//! already distinguishes (§6.1/§6.2) — including two the design doc calls
//! out for "real attention": a submit that times out *after* Orange already
//! has it, and a DLR that races the submit response it's nominally about.
//! It **cannot** prove anything about Orange's real behaviour — no real DLR
//! payload shape, no real `receiptRequest` honouring, no real handset. That
//! stays `docs/runbooks/36-handset-gate.adoc`'s job.
//!
//! # DLR delivery goes over real HTTP, deliberately
//!
//! `backends/apps/sms-gateway/src/dlr.rs` owns the real `POST /dlr/{providerKey}`
//! route, but `backends/apps/sms-gateway` is a binary crate with no `lib.rs`, so its
//! modules can't be imported from a test in another crate — the same
//! constraint `oidc_flow_live.rs` already documents (see AGENTS.md). This
//! file hand-rolls the same small handler (`dlr_handler` below, ~15 lines)
//! against a real `axum::serve` on an ephemeral port, so `sms-fake-orange`'s
//! DLR scheduler makes a genuine HTTP round trip into this system, not a
//! direct function call standing in for one.
//!
//! # Two kinds of test
//!
//! Scripted tests (a fixed [`sms_fake_orange::FaultPolicy::Scripted`]
//! sequence) assert one exact outcome each — the fault modes prioritised by
//! the design brief, one per test. The seeded chaos sweep
//! (`seeded_chaos_seed_*`) instead drives a batch of messages through a
//! weighted-random fault mix and asserts *invariants* over the result, not
//! a specific end state — see `run_seed`'s own doc for the list.
//!
//! Ignored by default, same convention as every other live suite in this
//! workspace:
//!
//! ```bash
//! cargo test -p sms-worker --test chaos_live_postgres -- --ignored
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use cratestack::CoolContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use sms_fake_orange::{
    DlrStatus, DlrStep, FakeOrange, FaultPolicy, Ledger, SubmitDecision, TokenPolicy,
};
use sms_provider::{RawCallback, SmsProvider};
use sms_provider_orange_cm::{OrangeCmConfig, OrangeCmProvider};
use sms_worker::WorkerContext;
use sms_worker::dispatch::tick;
use sms_worker::jobs::expire_stale::ExpireStale;

/// Every terminal `MessageState` — nothing leaves these (§2.10's own
/// comment: "terminality is data, not code").
const TERMINAL_STATES: [MessageState; 5] = [
    MessageState::delivered,
    MessageState::failed,
    MessageState::expired,
    MessageState::rejected,
    MessageState::cancelled,
];

const CHAOS_SENDER_NUMBER: &str = "+2370000";
/// Deliberately short — see `sms_fake_orange::fault`'s own module doc for
/// the delay budget every fault it schedules is chosen relative to. Real
/// production values (10s/30s) would make the deliberate-timeout fault
/// modes take real wall-clock seconds per call; this suite has no interest
/// in proving `reqwest`'s own timeout mechanics again, only in proving what
/// `dispatch`/`dlr` do once a timeout fires.
const CHAOS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const CHAOS_REQUEST_TIMEOUT: Duration = Duration::from_millis(150);

/// Serializes every test in this file — the same "intra-binary" reasoning
/// `dispatch_live_postgres.rs`'s own `TEST_MUTEX` documents: `dispatch`'s
/// claim loop selects candidates system-wide, and this test binary's
/// database is shared by every test in it for the lifetime of one `cargo
/// test` process. `sms-test-support`'s per-binary database means no other
/// *file* in this workspace can leave anything behind here — only tests
/// within this one file can, hence the guard plus [`clear_claimable_backlog`].
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "sms-worker-chaos-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-worker-chaos-test-owner".to_owned(),
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

/// Same fix, same reasoning as `dispatch_live_postgres.rs`'s own
/// `clear_claimable_backlog` — `dispatch::tick`'s claim query is global, so
/// a message an earlier test in this file left `accepted`/`queued`/`routed`
/// is exactly as claimable as the row the current test is about to seed,
/// and can land in the current test's own `wiremock`-backed fake instead of
/// the message it's tracking.
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

/// #122: `undelivered` joined the claimable set (`claim.rs`'s
/// `Claimable for Message::candidates()` now selects it for retry), so it
/// needs the same draining discipline as `accepted`/`queued`/`routed`
/// above, for the same reason — a leftover row from an earlier test in this
/// file is exactly as claimable as the row the current test is about to
/// seed, and this suite drives the real `tick()` loop repeatedly. Separate
/// loop, not folded into the one above: `cancelled` is unreachable from
/// `undelivered` (§2.10 has no such edge); `failed` is the legal terminal
/// edge this state actually has.
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

async fn isolated_db() -> Cratestack {
    let db = db().await;
    clear_claimable_backlog(&db).await;
    db
}

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "chaos test app".to_owned(),
            slug: format!("chaos-test-{}", unique_suffix()),
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

/// Seeds an active `Provider` plus a catch-all `Route` pointing at it, and
/// returns `(provider id, provider key)` — the id for the DLR route's own
/// `provider_row_id` correlation, the key for the caller's
/// `WorkerContext.providers` registry (#62: `dispatch::resolve_provider`
/// looks a routed message's provider back up by this exact string).
///
/// Only needs to disable *providers*, not routes — `build_harness` always
/// calls [`deactivate_every_active_provider`] immediately before this, so
/// every leftover route from an earlier test already points at a provider
/// this call just deactivated, and the routing engine excludes it as
/// `ProviderUnavailable` regardless of whether it's still `enabled`. See
/// `dispatch_live_postgres.rs`'s own `seed_routed_provider`/
/// `disable_every_route` for the shape of this same problem where that
/// guarantee doesn't hold.
async fn seed_active_provider(db: &Cratestack) -> (String, String) {
    let key: String = format!("chaos_test_{}", unique_suffix())
        .chars()
        .take(32)
        .collect();
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: key.clone(),
            displayName: "Chaos test provider".to_owned(),
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
            name: format!("chaos-test-route-{}", unique_suffix()),
            priority: 1000,
            weight: 1,
            enabled: true,
            matchOperator: None,
            matchClass: None,
            matchAppId: None,
            matchPrefix: None,
            providerId: provider.id.clone(),
            failoverRouteId: None,
        })
        .run(&owner())
        .await
        .expect("seeding a catch-all route");

    (provider.id, key)
}

/// `claim.rs::cheapest_active_provider` picks the cheapest **active**
/// `Provider` row across the *whole* table, with no tie-breaker beyond
/// Postgres's own arbitrary ordering — found live running this suite for
/// the first time: every test in this file seeds a provider at the same
/// fixed cost, and earlier tests' rows stay `active` (nothing deactivates
/// them), so a later test's message can route to an *earlier* test's
/// provider row instead of the one this test just created and wired its
/// own DLR route to. The DLR then correlates against the wrong
/// `provider_row_id` and is silently dropped as "no known message" — same
/// root cause, same fix `dispatch_live_postgres.rs`/`claim_live_postgres.rs`
/// already document for the identical shape.
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

async fn seed_message(
    db: &Cratestack,
    app_id: &str,
    max_attempts: i64,
    expires_at: DateTime<Utc>,
) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("chaos-test-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:chaos-test-{}", unique_suffix()),
            operator: OperatorCode::mtn,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            priority: 1000,
            body: Some("chaos suite test".to_owned()),
            bodyHash: "hmac-sha256-v1:chaos-test".to_owned(),
            bodyLength: 17,
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
            expiresAt: expires_at,
            submittedAt: None,
            finalizedAt: None,
            purgedAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the message")
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

/// Best-effort: forces an in-the-future `leaseUntil` into the past so the
/// next `tick()` treats a backed-off `queued` row as immediately claimable
/// again, without a real sleep. A version mismatch (the row moved on since
/// it was last read) just means the next poll of this same message in the
/// caller's own loop sees the newer state and decides again — not a fault.
async fn force_lease_past(db: &Cratestack, message: &Message) {
    let _ = db
        .message()
        .update(message.id.clone())
        .set(schema::UpdateMessageInput {
            leaseUntil: Some(Some(Utc::now() - ChronoDuration::seconds(1))),
            ..Default::default()
        })
        .if_match(message.version)
        .run(&sys())
        .await;
}

/// State shared by the hand-rolled DLR route below — see the module doc for
/// why this duplicates (rather than imports) `backends/apps/sms-gateway/src/dlr.rs`'s
/// own handler.
#[derive(Clone)]
struct ChaosDlrState {
    db: Cratestack,
    provider: Arc<dyn SmsProvider>,
    provider_row_id: String,
}

async fn dlr_handler(
    Path(provider_key): Path<String>,
    State(state): State<ChaosDlrState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if provider_key != state.provider.key() {
        return StatusCode::NOT_FOUND;
    }
    let raw = RawCallback {
        headers: headers
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    value.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect(),
        body: body.to_vec(),
    };
    match sms_api::dlr::ingest(
        &state.db,
        &sys(),
        state.provider.as_ref(),
        &state.provider_row_id,
        &raw,
    )
    .await
    {
        Ok(()) => StatusCode::ACCEPTED,
        Err(error) => {
            tracing::warn!(provider_key, %error, "chaos DLR route: ingest rejected the callback body");
            StatusCode::BAD_REQUEST
        }
    }
}

/// Starts a real `axum::serve` on an ephemeral loopback port, mounting the
/// same `POST /dlr/{providerKey}` shape `backends/apps/sms-gateway/src/dlr.rs` mounts
/// in production. Returns the full URL to this provider's own route
/// (`.../dlr/orange_cm`) — what `FakeOrange::start`'s `dlr_endpoint`
/// parameter expects.
async fn spawn_dlr_server(
    db: Cratestack,
    provider: Arc<dyn SmsProvider>,
    provider_row_id: String,
) -> String {
    let key = provider.key().to_owned();
    let state = ChaosDlrState {
        db,
        provider,
        provider_row_id,
    };
    let app: Router = Router::new()
        .route("/dlr/{providerKey}", post(dlr_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding an ephemeral port for the chaos DLR server");
    let addr = listener.local_addr().expect("reading the bound address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("chaos DLR server crashed");
    });

    format!("http://{addr}/dlr/{key}")
}

fn orange_config(base_url: String) -> OrangeCmConfig {
    OrangeCmConfig {
        client_id: "chaos-client".to_owned(),
        client_secret: "chaos-secret".to_owned(),
        sender_number: CHAOS_SENDER_NUMBER.to_owned(),
        base_url,
        dlr_notify_url: None,
        connect_timeout: CHAOS_CONNECT_TIMEOUT,
        request_timeout: CHAOS_REQUEST_TIMEOUT,
    }
}

/// Everything one chaos test needs: an isolated database, an active
/// provider row, a fresh app, a real in-process DLR route, and
/// `dispatch`'s own `WorkerContext` pointed at [`FakeOrange`].
struct Harness {
    db: Cratestack,
    sys: CoolContext,
    app_id: String,
    ctx: WorkerContext,
    fake: FakeOrange,
}

/// `OrangeCmProvider::parse_dlr` ignores `self` entirely (it delegates to a
/// free function over the raw bytes — see that crate's own `lib.rs`), so
/// the DLR route's own provider instance never needs a working `base_url`;
/// only the one `dispatch` submits through does. Building two instances,
/// not sharing one, sidesteps an otherwise-real ordering problem: the DLR
/// route must exist *before* `FakeOrange::start` (which needs the route's
/// URL), but the submit-side provider needs `FakeOrange::base_url()`, which
/// doesn't exist until *after* the fake starts.
async fn build_harness(policy: FaultPolicy, token_policy: TokenPolicy) -> Harness {
    let db = isolated_db().await;
    let sys = sys();
    deactivate_every_active_provider(&db).await;
    let (provider_row_id, provider_key) = seed_active_provider(&db).await;
    let app_id = seed_app(&db).await;

    let dlr_route_provider: Arc<dyn SmsProvider> = Arc::new(OrangeCmProvider::new(orange_config(
        "http://127.0.0.1:1".to_owned(),
    )));
    let dlr_endpoint = spawn_dlr_server(db.clone(), dlr_route_provider, provider_row_id).await;

    let fake = FakeOrange::start(policy, token_policy, dlr_endpoint, CHAOS_SENDER_NUMBER).await;

    let submit_provider: Arc<dyn SmsProvider> =
        Arc::new(OrangeCmProvider::new(orange_config(fake.base_url())));
    let ctx = WorkerContext {
        db: db.clone(),
        providers: Arc::new(std::collections::HashMap::from([(
            provider_key,
            submit_provider,
        )])),
    };

    Harness {
        db,
        sys,
        app_id,
        ctx,
        fake,
    }
}

// ---------------------------------------------------------------------
// Scripted tests — one exact outcome each, the fault modes the design
// brief prioritises. Connection-level nastiness (RST mid-response, refused
// connections, byte-dribble) is out of scope for this PR — see
// `sms_fake_orange::fault`'s own module doc for why `wiremock` can't model
// it without connection-level control this crate doesn't take on, and
// `sms-provider-orange-cm`'s own `a_connect_refusal_is_still_unavailable`
// for where the connect-refused half is already covered directly.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn no_dlr_ever_arrives_then_expire_stale_reaps_the_submitted_message() {
    let _guard = TEST_MUTEX.lock().await;
    let harness = build_harness(
        FaultPolicy::scripted([SubmitDecision::accepted()]),
        TokenPolicy::Always,
    )
    .await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // accepted -> queued
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // queued -> routed -> submitted

    let after_submit = reload(&harness.db, &seeded.id).await;
    assert_eq!(after_submit.state, MessageState::submitted);
    assert_eq!(
        harness.fake.ledger().pending_dlrs(),
        0,
        "no DLR was ever scheduled"
    );

    ExpireStale
        .run_at(
            &harness.db,
            &harness.sys,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await
        .expect("expire_stale");

    let expired = reload(&harness.db, &seeded.id).await;
    assert_eq!(expired.state, MessageState::expired);
}

/// The scenario the design brief calls out for real attention: the DLR
/// fires as soon as the fake receives the submit request, while the submit
/// HTTP response is deliberately held open — so `Message.providerMessageRef`
/// / `providerMessageRefAlt` are still `NULL` when the DLR arrives. It must
/// correlate to nothing and be silently dropped; the message reaches
/// `submitted` normally afterward (`write_submitted` still runs) and, with no
/// further DLR ever coming, is reaped by `expire_stale` — not stuck, not
/// wrongly `delivered`.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_dlr_racing_the_submit_response_is_dropped_then_the_message_expires() {
    let _guard = TEST_MUTEX.lock().await;
    let racing_decision = SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
        Duration::ZERO,
        DlrStatus::Delivered,
    )])
    .response_delay(Duration::from_millis(120));
    let harness = build_harness(
        FaultPolicy::scripted([racing_decision]),
        TokenPolicy::Always,
    )
    .await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // accepted -> queued
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // queued -> routed -> submitted (after the race)

    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(2))
            .await,
        "the racing DLR's own HTTP round trip never settled"
    );

    let after_submit = reload(&harness.db, &seeded.id).await;
    assert_eq!(
        after_submit.state,
        MessageState::submitted,
        "a DLR racing ahead of the submit response must correlate to nothing and be dropped, \
         not resolve the message to delivered"
    );

    ExpireStale
        .run_at(
            &harness.db,
            &harness.sys,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await
        .expect("expire_stale");

    let expired = reload(&harness.db, &seeded.id).await;
    assert_eq!(
        expired.state,
        MessageState::expired,
        "the message must still reach a real terminal state"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_duplicate_delivered_dlr_is_idempotent() {
    let _guard = TEST_MUTEX.lock().await;
    let dlrs = vec![
        DlrStep::after(Duration::from_millis(30), DlrStatus::Delivered),
        DlrStep::after(Duration::from_millis(120), DlrStatus::Delivered),
    ];
    let harness = build_harness(
        FaultPolicy::scripted([SubmitDecision::accepted_with_dlrs(dlrs)]),
        TokenPolicy::Always,
    )
    .await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");
    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(2))
            .await
    );

    let after = reload(&harness.db, &seeded.id).await;
    assert_eq!(after.state, MessageState::delivered);

    let receipts = harness
        .db
        .delivery_receipt()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::delivery_receipt::messageId().eq(seeded.id.clone()),
        ))
        .run(&harness.sys)
        .await
        .expect("listing delivery receipts");
    assert_eq!(
        receipts.len(),
        2,
        "both DLRs must be recorded, even the one that was a same-state no-op"
    );
}

/// `undelivered -> delivered` isn't a legal edge (§2.10) — a `delivered` DLR
/// arriving after an earlier `Failed` DLR already moved the message to
/// `undelivered` must be rejected by the trigger, not silently applied.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_out_of_order_dlr_proposing_an_illegal_transition_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let dlrs = vec![
        DlrStep::after(Duration::from_millis(30), DlrStatus::Failed),
        DlrStep::after(Duration::from_millis(120), DlrStatus::Delivered),
    ];
    let harness = build_harness(
        FaultPolicy::scripted([SubmitDecision::accepted_with_dlrs(dlrs)]),
        TokenPolicy::Always,
    )
    .await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");
    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(2))
            .await
    );

    let after = reload(&harness.db, &seeded.id).await;
    assert_eq!(
        after.state,
        MessageState::undelivered,
        "the first (legal) transition must apply; the second (illegal) one must be refused, not \
         silently override it"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_dlr_for_an_unknown_reference_is_dropped_then_the_message_expires() {
    let _guard = TEST_MUTEX.lock().await;
    let dlrs = vec![DlrStep::for_unknown_ref(
        Duration::from_millis(30),
        DlrStatus::Delivered,
        "not-a-real-message-id",
    )];
    let harness = build_harness(
        FaultPolicy::scripted([SubmitDecision::accepted_with_dlrs(dlrs)]),
        TokenPolicy::Always,
    )
    .await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");
    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(2))
            .await
    );

    let after = reload(&harness.db, &seeded.id).await;
    assert_eq!(
        after.state,
        MessageState::submitted,
        "an unrelated reference must not resolve this message"
    );

    ExpireStale
        .run_at(
            &harness.db,
            &harness.sys,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await
        .expect("expire_stale");
    let expired = reload(&harness.db, &seeded.id).await;
    assert_eq!(expired.state, MessageState::expired);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_rate_limited_submit_recovers_on_retry() {
    let _guard = TEST_MUTEX.lock().await;
    let decisions = [
        SubmitDecision::rate_limited(),
        SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
            Duration::from_millis(20),
            DlrStatus::Delivered,
        )]),
    ];
    let harness = build_harness(FaultPolicy::scripted(decisions), TokenPolicy::Always).await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // accepted -> queued
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // queued -> routed -> (429) -> queued

    let after_429 = reload(&harness.db, &seeded.id).await;
    assert_eq!(after_429.state, MessageState::queued);
    force_lease_past(&harness.db, &after_429).await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // queued -> routed -> submitted
    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(2))
            .await
    );

    let resolved = reload(&harness.db, &seeded.id).await;
    assert_eq!(resolved.state, MessageState::delivered);
    assert_eq!(harness.fake.ledger().submit_count(&seeded.id), 2);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_rejected_submit_fails_the_message_outright() {
    let _guard = TEST_MUTEX.lock().await;
    let harness = build_harness(
        FaultPolicy::scripted([SubmitDecision::rejected()]),
        TokenPolicy::Always,
    )
    .await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");

    let after = reload(&harness.db, &seeded.id).await;
    assert_eq!(after.state, MessageState::failed);
    assert_eq!(after.attempts, 1);
}

/// #95/#119's territory: `dispatch`'s own `write_transition` stamps
/// `providerMessageRefAlt = Message.id` even when the submit itself
/// "failed" with a malformed body — because Orange's `201` means it was
/// genuinely accepted regardless of what the body looked like.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_malformed_201_body_lands_in_uncertain_and_is_never_resubmitted() {
    let _guard = TEST_MUTEX.lock().await;
    let harness = build_harness(
        FaultPolicy::scripted([SubmitDecision::malformed_body()]),
        TokenPolicy::Always,
    )
    .await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");

    let after = reload(&harness.db, &seeded.id).await;
    assert_eq!(after.state, MessageState::uncertain);

    for _ in 0..3 {
        tick(&harness.ctx, &harness.sys, "chaos-worker")
            .await
            .expect("tick");
    }
    assert_eq!(
        harness.fake.ledger().submit_count(&seeded.id),
        1,
        "uncertain must never be resubmitted"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_201_with_a_missing_resource_url_lands_in_uncertain_and_is_never_resubmitted() {
    let _guard = TEST_MUTEX.lock().await;
    let harness = build_harness(
        FaultPolicy::scripted([SubmitDecision::missing_resource_url()]),
        TokenPolicy::Always,
    )
    .await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick");

    let after = reload(&harness.db, &seeded.id).await;
    assert_eq!(after.state, MessageState::uncertain);

    for _ in 0..3 {
        tick(&harness.ctx, &harness.sys, "chaos-worker")
            .await
            .expect("tick");
    }
    assert_eq!(harness.fake.ledger().submit_count(&seeded.id), 1);
}

/// A submit that Orange genuinely accepts but never answers within
/// `dispatch`'s own `request_timeout` — `Indeterminate`, `routed ->
/// uncertain`. Modelled here as a `201` with `response_delay` set well past
/// [`CHAOS_REQUEST_TIMEOUT`]; a later DLR still resolves it, proving the
/// loop `#119`'s own guarantee depends on: `providerMessageRefAlt` really
/// was recorded at timeout time.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_submit_that_times_out_after_orange_accepted_it_is_never_resubmitted_and_still_resolves()
{
    let _guard = TEST_MUTEX.lock().await;
    let decision = SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
        Duration::from_millis(500),
        DlrStatus::Delivered,
    )])
    .response_delay(Duration::from_millis(300));
    let harness = build_harness(FaultPolicy::scripted([decision]), TokenPolicy::Always).await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // accepted -> queued
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // queued -> routed -> (timeout) -> uncertain

    let uncertain = reload(&harness.db, &seeded.id).await;
    assert_eq!(uncertain.state, MessageState::uncertain);
    assert_eq!(uncertain.providerMessageRefAlt, Some(seeded.id.clone()));

    for _ in 0..3 {
        tick(&harness.ctx, &harness.sys, "chaos-worker")
            .await
            .expect("tick");
    }
    assert_eq!(
        harness.fake.ledger().submit_count(&seeded.id),
        1,
        "uncertain must never be resubmitted, even across several more polls"
    );

    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(2))
            .await
    );
    let resolved = reload(&harness.db, &seeded.id).await;
    assert_eq!(
        resolved.state,
        MessageState::delivered,
        "a DLR arriving after the fact must still resolve the message via providerMessageRefAlt"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_token_endpoint_401_fails_the_message_as_permanent() {
    let _guard = TEST_MUTEX.lock().await;
    let harness = build_harness(FaultPolicy::scripted([]), TokenPolicy::AlwaysUnauthorized).await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // accepted -> queued
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // queued -> routed -> (401 at token) -> failed

    let after = reload(&harness.db, &seeded.id).await;
    assert_eq!(after.state, MessageState::failed);
    assert_eq!(
        harness.fake.ledger().submits().len(),
        0,
        "a token failure never reaches the submit endpoint at all"
    );
}

/// #122's own regression test: a message that receives exactly one
/// retryable-failure DLR must not sit in `undelivered` forever — it has to
/// come back around and be retried. `force_lease_past` stands in for real
/// wall-clock time, the same way the seeded sweep does, so this doesn't
/// need to sleep out `undelivered_retry_backoff`'s real delay.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_undelivered_message_is_retried_and_reaches_delivered_on_the_next_attempt() {
    let _guard = TEST_MUTEX.lock().await;
    let harness = build_harness(
        FaultPolicy::scripted([
            SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
                Duration::from_millis(30),
                DlrStatus::Failed,
            )]),
            SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
                Duration::from_millis(30),
                DlrStatus::Delivered,
            )]),
        ]),
        TokenPolicy::Always,
    )
    .await;
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        3,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // accepted -> queued
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // queued -> routed -> submitted
    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(2))
            .await
    );

    let first_failure = reload(&harness.db, &seeded.id).await;
    assert_eq!(
        first_failure.state,
        MessageState::undelivered,
        "the first retryable-failure DLR must land the message in undelivered"
    );

    // Stand in for `undelivered_retry_backoff`'s real delay — same
    // mechanism the seeded sweep below uses.
    force_lease_past(&harness.db, &first_failure).await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // undelivered -> queued (the retry, #122)
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // queued -> routed -> submitted, the second attempt
    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(2))
            .await
    );

    let resolved = reload(&harness.db, &seeded.id).await;
    assert_eq!(
        resolved.state,
        MessageState::delivered,
        "the bug this test guards against: before #122, nothing ever drove \
         undelivered -> queued, so this message would still be sitting in undelivered"
    );
    assert_eq!(
        harness.fake.ledger().submit_count(&seeded.id),
        2,
        "exactly one retry — the first submit, then the one this test proves happens"
    );
}

/// The bounded half of the same fix: a message that keeps failing must
/// still reach a terminal state once `maxAttempts` is exhausted, not retry
/// forever.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn an_undelivered_message_at_max_attempts_fails_instead_of_retrying_forever() {
    let _guard = TEST_MUTEX.lock().await;
    let harness = build_harness(
        FaultPolicy::scripted([SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
            Duration::from_millis(30),
            DlrStatus::Failed,
        )])]),
        TokenPolicy::Always,
    )
    .await;
    // maxAttempts: 1 — the one submission this message is allowed to make
    // happens on the very first `queued -> routed` hop, so by the time the
    // retryable-failure DLR lands it has already exhausted its budget.
    let seeded = seed_message(
        &harness.db,
        &harness.app_id,
        1,
        Utc::now() + ChronoDuration::hours(1),
    )
    .await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // accepted -> queued
    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // queued -> routed -> submitted
    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(2))
            .await
    );

    let undelivered = reload(&harness.db, &seeded.id).await;
    assert_eq!(undelivered.state, MessageState::undelivered);
    force_lease_past(&harness.db, &undelivered).await;

    tick(&harness.ctx, &harness.sys, "chaos-worker")
        .await
        .expect("tick"); // undelivered -> failed: max attempts (§7.4)

    let after = reload(&harness.db, &seeded.id).await;
    assert_eq!(after.state, MessageState::failed);
    assert_eq!(
        harness.fake.ledger().submit_count(&seeded.id),
        1,
        "a message at max attempts must never be resubmitted"
    );
}

// ---------------------------------------------------------------------
// Seeded chaos sweep — a fixed, small seed set so every PR gets
// deterministic, reproducible regression coverage. A failing seed is
// always replayable: it's named in the test's own function name and in
// every assertion message below.
// ---------------------------------------------------------------------

const CHAOS_SEEDS: [u64; 5] = [1, 2, 3, 4, 5];
const MESSAGES_PER_SEED: usize = 8;
const MAX_TICKS: usize = 40;

/// Drives [`MESSAGES_PER_SEED`] messages through `dispatch::tick` under a
/// [`FaultPolicy::Seeded`] policy until none remain claimable (or
/// [`MAX_TICKS`] is exhausted, itself an assertion — see below), lets the
/// fake's own DLR-delivery tasks settle, forces `expire_stale` far enough
/// into the future to resolve every `submitted`/`uncertain`/`undelivered`
/// row regardless of the real 6h grace or backoff, then sweeps every seeded
/// message and asserts:
///
/// - **no message is lost** — every one ends in a real terminal state.
///   Before #122 this exempted `undelivered` as a known, accepted gap
///   (nothing drove `undelivered -> queued`, and `expire_stale` didn't reap
///   it either, so a message that received exactly one retryable-failure
///   DLR and no follow-up sat there forever); `claim.rs`'s `candidates()`
///   now selects `undelivered` for retry and `expire_stale` now reaps a row
///   whose `expiresAt` elapses before its retry budget does, so this sweep
///   enforces the stronger invariant directly instead of carving the gap
///   out;
/// - **no message is still claimable** — `accepted`/`queued`/`routed`/
///   `undelivered` must be fully drained by the tick loop itself, never
///   left over for the sweep to paper over;
/// - **`attempts` never exceeds `maxAttempts`**;
/// - **a message that went `uncertain` via `Indeterminate` is never
///   submitted again** — checked against the fake's own ledger, not the
///   database, so it's a check on what Orange actually received, not on
///   what this system *thinks* it sent (see
///   `assert_never_resubmitted_after_indeterminate`).
async fn run_seed(seed: u64) {
    let harness = build_harness(FaultPolicy::seeded(seed), TokenPolicy::Always).await;
    let mut seeded_ids = Vec::with_capacity(MESSAGES_PER_SEED);
    for _ in 0..MESSAGES_PER_SEED {
        let message = seed_message(
            &harness.db,
            &harness.app_id,
            2,
            Utc::now() + ChronoDuration::hours(1),
        )
        .await;
        seeded_ids.push(message.id);
    }

    let mut ticks_used = 0;
    for _ in 0..MAX_TICKS {
        ticks_used += 1;
        tick(&harness.ctx, &harness.sys, &format!("chaos-seed-{seed}"))
            .await
            .unwrap_or_else(|error| panic!("seed {seed}: tick failed: {error}"));

        let mut still_claimable = false;
        for id in &seeded_ids {
            let message = reload(&harness.db, id).await;
            if matches!(
                message.state,
                MessageState::accepted
                    | MessageState::queued
                    | MessageState::routed
                    | MessageState::undelivered
            ) {
                still_claimable = true;
                // `undelivered`'s own backoff lease (`sms_api::dlr`'s
                // `undelivered_retry_backoff`, up to 30 minutes) is exactly
                // as real a wait as `routed -> queued`'s backoff — force it
                // past the same way, so this sweep proves the retry path
                // itself rather than timing out waiting on real wall clock.
                if message.leaseUntil.is_some_and(|until| until > Utc::now()) {
                    force_lease_past(&harness.db, &message).await;
                }
            }
        }
        if !still_claimable {
            break;
        }
    }

    for id in &seeded_ids {
        let message = reload(&harness.db, id).await;
        assert!(
            !matches!(
                message.state,
                MessageState::accepted
                    | MessageState::queued
                    | MessageState::routed
                    | MessageState::undelivered
            ),
            "seed {seed}: message {id} still claimable (state {:?}) after {ticks_used} ticks — \
             the sweep's own bounded-attempts assumption was violated",
            message.state
        );
    }

    assert!(
        harness
            .fake
            .ledger()
            .wait_for_dlrs_to_settle(Duration::from_secs(3))
            .await,
        "seed {seed}: fake orange's own DLR-delivery tasks never settled within 3s"
    );

    ExpireStale
        .run_at(
            &harness.db,
            &harness.sys,
            Utc::now() + ChronoDuration::hours(7),
        )
        .await
        .unwrap_or_else(|error| panic!("seed {seed}: expire_stale run_at failed: {error}"));

    for id in &seeded_ids {
        let message = reload(&harness.db, id).await;
        assert!(
            TERMINAL_STATES.contains(&message.state),
            "seed {seed}: message {id} ended in {:?}, not a real terminal state — the message \
             was effectively lost (#122: `undelivered` is no longer an accepted outcome here, \
             it must now always resolve onward)",
            message.state
        );
        assert!(
            message.attempts <= message.maxAttempts,
            "seed {seed}: message {id} attempts {} exceeded maxAttempts {}",
            message.attempts,
            message.maxAttempts
        );
    }

    assert_never_resubmitted_after_indeterminate(&harness.fake.ledger(), seed);
}

/// For every reference the fake ever received a submit call for: if any
/// call's own `response_delay` was at least [`CHAOS_REQUEST_TIMEOUT`] (the
/// shape that reads as `Indeterminate` to `dispatch`'s configured client),
/// that call must be the *last* one this reference was ever submitted with
/// — `#119`'s own guarantee, checked from the provider's side of the wire.
fn assert_never_resubmitted_after_indeterminate(ledger: &Ledger, seed: u64) {
    let mut by_reference: HashMap<String, Vec<sms_fake_orange::SubmitRecord>> = HashMap::new();
    for record in ledger.submits() {
        by_reference
            .entry(record.reference.clone())
            .or_default()
            .push(record);
    }
    for (reference, mut records) in by_reference {
        records.sort_by_key(|record| record.at);
        if let Some(first_indeterminate) = records
            .iter()
            .position(|record| record.response_delay >= CHAOS_REQUEST_TIMEOUT)
        {
            assert_eq!(
                first_indeterminate,
                records.len() - 1,
                "seed {seed}: message {reference} was submitted again after an Indeterminate-shaped \
                 submit call (response_delay >= the client's own request_timeout) — this must \
                 never happen, it risks a duplicate real SMS"
            );
        }
    }
}

macro_rules! seeded_chaos_test {
    ($name:ident, $seed:expr) => {
        #[tokio::test]
        #[ignore = "needs a live, fully migrated Postgres — see module docs"]
        async fn $name() {
            let _guard = TEST_MUTEX.lock().await;
            run_seed($seed).await;
        }
    };
}

seeded_chaos_test!(seeded_chaos_seed_1, CHAOS_SEEDS[0]);
seeded_chaos_test!(seeded_chaos_seed_2, CHAOS_SEEDS[1]);
seeded_chaos_test!(seeded_chaos_seed_3, CHAOS_SEEDS[2]);
seeded_chaos_test!(seeded_chaos_seed_4, CHAOS_SEEDS[3]);
seeded_chaos_test!(seeded_chaos_seed_5, CHAOS_SEEDS[4]);
