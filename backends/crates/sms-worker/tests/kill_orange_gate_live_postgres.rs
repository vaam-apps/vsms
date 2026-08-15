//! #65 — Milestone 5's acceptance gate: "Take Orange down in staging.
//! Assert MTN traffic is unaffected, Orange-destined traffic fails over
//! cleanly, nothing double-sends, and the circuit breaker reopens when
//! Orange returns."
//!
//! Same split as `docs/runbooks/36-handset-gate.adoc` (M2's own gate, cited
//! by AGENTS.md as the model to follow): the automatable half is proven
//! here, rigorously, against real processes; the half that genuinely needs
//! a real staging deployment and a real Orange/aggregator account is
//! `docs/runbooks/65-kill-orange-gate.adoc`. Which is which is stated
//! plainly in both places — this suite proves the *mechanism* (routing,
//! failover, the circuit breaker) holds under a real transport-level
//! outage; it cannot prove Orange's real failure modes match what's
//! injected here, the same honesty ledger `sms_fake_orange`'s own module
//! doc already carries for the M2/M3 chaos suite.
//!
//! # What makes this a "kill", not a synthetic error
//!
//! Every M5 failover/circuit-breaker test that came before this one
//! (`dispatch_live_postgres.rs`'s `a_permanent_failure_fails_over_...`,
//! `an_open_circuit_routes_new_messages_...`) uses a hand-rolled
//! `AlwaysErr`/`AlwaysOk` `SmsProvider` that returns a synthetic
//! `ProviderError` directly — real coverage of the state-machine reaction,
//! but not of the transport failure itself. This suite instead:
//!
//! - Talks to Orange through the **real** `sms_provider_orange_cm::
//!   OrangeCmProvider` adapter, and to MTN through the **real**
//!   `sms_provider_mtn::MtnAggregatorProvider` adapter (#61) — not a fake
//!   `SmsProvider` impl for either.
//! - "Kills" Orange by dropping a real, running `sms_fake_orange::FakeOrange`
//!   HTTP server, so the very next submit attempt gets a genuine OS-level
//!   `ECONNREFUSED` through `OrangeCmProvider::submit` — real
//!   `reqwest`/`classify_transport_error` behaviour, not an injected
//!   `ProviderError::Unavailable`.
//! - "Revives" Orange by starting a **second** `FakeOrange` bound to the
//!   exact same local port the first one used — see [`reuseaddr_listener`]'s
//!   own doc for why that needs `SO_REUSEADDR` and isn't just
//!   `TcpListener::bind` again — so the very same, already-constructed
//!   `OrangeCmProvider` (its `base_url` is fixed at construction, exactly
//!   like every other adapter in this workspace) resumes talking to
//!   "Orange" without needing a second adapter instance or any config
//!   reload.
//! - Proves "nothing double-sends" from **the providers' own request
//!   logs** — `FakeOrange::ledger()` for Orange, `MockServer::
//!   received_requests()` for MTN — never from this system's own
//!   `Message` rows. The database records what this system *believes* it
//!   did; a provider's own ledger records what it actually received. The
//!   distinction matters most precisely in the failure mode this gate
//!   exists to rule out.
//!
//! # Why one scenario, not four
//!
//! The issue's four clauses are not independent properties of four
//! different mechanisms — they are four things that must all be true of
//! **one realistic timeline**: healthy, killed, and revived. Splitting
//! them into four unrelated tests would each need to reconstruct that
//! timeline anyway, and would lose the property that actually matters most
//! for a *gate*: that all four hold *together*, in the order a real outage
//! would produce them. `orange_outage_fails_over_mtn_stays_up_nothing_
//! double_sends_and_the_breaker_reopens` is the whole story, phase by
//! phase, each phase's assertions labelled with which of the issue's four
//! clauses it proves.
//!
//! Ignored by default, same convention as every other live suite in this
//! workspace:
//!
//! ```bash
//! cargo test -p sms-worker --test kill_orange_gate_live_postgres -- --ignored
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode,
};
use sms_fake_orange::{FakeOrange, FaultPolicy, SubmitDecision, TokenPolicy};
use sms_provider::SmsProvider;
use sms_provider_mtn::{MtnAggregatorConfig, MtnAggregatorProvider};
use sms_provider_orange_cm::{OrangeCmConfig, OrangeCmProvider};
use sms_worker::dispatch::tick;
use sms_worker::WorkerContext;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Same reasoning as every other live suite's own `TEST_MUTEX` — see
/// `dispatch_live_postgres.rs`'s own doc for the full history (a claim
/// loop that selects candidates system-wide, racing concurrently-run tests
/// in the same binary's never-reset-between-runs database). This binary
/// has exactly one live test in it today, so this mutex is currently
/// inert in practice — kept anyway, both as a guard against a future test
/// being added to this file without re-deriving the reasoning, and because
/// `isolated_db`'s own draining pass assumes it runs under this lock.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "sms-worker-kill-orange-gate".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-worker-kill-orange-gate-owner".to_owned(),
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
/// `clear_claimable_backlog` — copied here rather than shared, matching
/// every other live-test binary's own established convention (each test
/// binary is independent; see AGENTS.md on why this duplication is
/// accepted, not a shortcut).
async fn clear_claimable_backlog(db: &Cratestack) {
    const BATCH: usize = 500;
    let sys = sys();
    loop {
        let backlog = db
            .message()
            .find_many()
            .where_expr(FilterExpr::from(schema::message::state().in_([
                MessageState::accepted,
                MessageState::queued,
                MessageState::routed,
            ])))
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
}

async fn isolated_db() -> Cratestack {
    let db = db().await;
    clear_claimable_backlog(&db).await;
    db
}

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "kill-orange-gate app".to_owned(),
            slug: format!("kill-orange-gate-{}", unique_suffix()),
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

/// Same isolation reasoning as `dispatch_live_postgres.rs`'s own
/// `disable_every_route`/`deactivate_every_active_provider` — this binary
/// has one test today, but the guarantee ("exactly this test's own two
/// routes are enabled, exactly this test's own two providers are active")
/// must hold regardless of what any future test in this file, or a
/// previous run against this never-reset-between-runs database, left
/// behind.
async fn clear_routing_state(db: &Cratestack) {
    let enabled = db
        .route()
        .find_many()
        .where_expr(FilterExpr::from(schema::route::enabled().is_true()))
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
            .if_match(route.version)
            .run(&owner())
            .await
            .expect("disabling a leftover enabled route");
    }

    let active = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
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
            .if_match(provider.version)
            .run(&owner())
            .await
            .expect("deactivating a leftover active provider");
    }
}

struct GateFixture {
    orange_id: String,
    orange_key: String,
    mtn_id: String,
    mtn_key: String,
}

/// One active `Provider` row, labelled — pulled out to module scope for
/// the same `clippy::too_many_lines` reason `dispatch_live_postgres.rs`'s
/// own `seed_one_active_provider` already documents, and reused for both
/// halves of [`seed_gate_fixture`] rather than writing this twice.
async fn seed_active_provider_row(
    db: &Cratestack,
    label: &str,
    kind: schema::ProviderKind,
    max_tps: f64,
    cost_per_segment_xaf: &str,
) -> schema::Provider {
    let key: String = format!("kill_gate_{label}_{}", unique_suffix())
        .chars()
        .take(32)
        .collect();
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key,
            displayName: format!("Kill-orange-gate {label}"),
            kind,
            config: "{}".to_owned(),
            credentialRef: "vault://test".to_owned(),
            maxTps: max_tps,
            maxDailySubmissions: 1000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: cost_per_segment_xaf.parse().unwrap(),
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

/// One enabled `Route` row — pulled out for the same reason as
/// [`seed_active_provider_row`] above.
async fn seed_route_row(
    db: &Cratestack,
    label: &str,
    priority: i64,
    match_operator: Option<OperatorCode>,
    provider_id: String,
) {
    db.route()
        .create(schema::CreateRouteInput {
            name: format!("kill-gate-{label}-{}", unique_suffix()),
            priority,
            weight: 1,
            enabled: true,
            matchOperator: match_operator,
            matchClass: None,
            matchAppId: None,
            matchPrefix: None,
            providerId: provider_id,
            failoverRouteId: None,
        })
        .run(&owner())
        .await
        .expect("seeding a route");
}

/// Two active `Provider`s and two enabled `Route`s, shaped after how a real
/// deployment would route Orange- and MTN-bound traffic: an
/// operator-scoped Orange route at the higher priority (so Orange traffic
/// prefers Orange when it's healthy), and a wildcard MTN route at a lower
/// priority that accepts *any* operator — modelling MTN-via-aggregator
/// capacity that isn't restricted to MTN's own network (#61's own module
/// doc: this is bought capacity, not a same-network requirement). This
/// wildcard is what makes Orange-destined failover to MTN possible at all;
/// it is also what makes an MTN-destined message never even consider
/// Orange in the first place — the strongest form of "unaffected" this
/// fixture can express.
async fn seed_gate_fixture(db: &Cratestack) -> GateFixture {
    clear_routing_state(db).await;

    let orange = seed_active_provider_row(
        db,
        "orange",
        schema::ProviderKind::orange_cm_http,
        5.0,
        "15",
    )
    .await;
    let mtn = seed_active_provider_row(db, "mtn", schema::ProviderKind::mtn_http, 20.0, "12").await;

    seed_route_row(
        db,
        "orange-primary",
        1000,
        Some(OperatorCode::orange),
        orange.id.clone(),
    )
    .await;
    seed_route_row(db, "mtn-catchall", 500, None, mtn.id.clone()).await;

    GateFixture {
        orange_id: orange.id,
        orange_key: orange.key,
        mtn_id: mtn.id,
        mtn_key: mtn.key,
    }
}

async fn seed_message(db: &Cratestack, app_id: &str, operator: OperatorCode) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("kill-orange-gate-{}", unique_suffix())),
            msisdn: "+237677123456".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:kill-orange-gate-{}", unique_suffix()),
            operator,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::otp,
            // Max priority — this database is never reset between runs;
            // see the identical comment in every sibling live suite's own
            // seed_message.
            priority: 1000,
            body: Some("kill-orange gate test".to_owned()),
            bodyHash: "hmac-sha256-v1:kill-orange-gate".to_owned(),
            bodyLength: 21,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: None,
            providerId: None,
            providerMessageRef: None,
            providerMessageRefAlt: None,
            excludedRouteIds: None,
            maxAttempts: 5,
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

async fn reload_message(db: &Cratestack, id: &str) -> Message {
    db.message()
        .find_many()
        .where_expr(FilterExpr::from(schema::message::id().eq(id.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("reloading the message")
        .into_iter()
        .next()
        .expect("the message still exists")
}

async fn reload_provider(db: &Cratestack, id: &str) -> schema::Provider {
    db.provider()
        .find_many()
        .where_expr(FilterExpr::from(schema::provider::id().eq(id.to_owned())))
        .limit(1)
        .run(&sys())
        .await
        .expect("reloading the provider")
        .into_iter()
        .next()
        .expect("the provider still exists")
}

/// Binds `addr` with `SO_REUSEADDR` set before `bind()`, unlike plain
/// `std::net::TcpListener::bind`. Needed for exactly one thing in this
/// suite: rebinding on the *same* port a previous listener (dropped, to
/// model Orange going down) used, once real HTTP traffic has actually
/// flowed through it — every accepted-then-closed connection leaves a
/// `TIME_WAIT` entry keyed by local port, and macOS/Linux both refuse a
/// fresh `bind()` to that port (`EADDRINUSE`) while any such entry exists,
/// unless the new socket opts in via `SO_REUSEADDR`. Confirmed empirically
/// before writing this suite, not assumed from platform documentation: a
/// throwaway Rust program (kept out of this repo, only reported in the PR
/// description) reproduced exactly this — a real HTTP round trip on an
/// ephemeral port, then a plain `TcpListener::bind` on that same port
/// failing `EADDRINUSE`, and a `SO_REUSEADDR` bind on the same port
/// succeeding immediately, before serving a second, genuinely new
/// listener.
///
/// `wiremock`'s own `run_server` (`hyper.rs`) calls
/// `listener.set_nonblocking(true)` itself before wrapping this in a
/// tokio listener, so this function doesn't need to.
fn reuseaddr_listener(addr: &str) -> std::net::TcpListener {
    use socket2::{Domain, Socket, Type};
    let socket_addr: std::net::SocketAddr = addr.parse().expect("a valid socket address");
    let socket = Socket::new(Domain::for_address(socket_addr), Type::STREAM, None)
        .expect("creating a raw TCP socket");
    socket
        .set_reuse_address(true)
        .expect("SO_REUSEADDR must be settable on a freshly created socket");
    socket
        .bind(&socket_addr.into())
        .expect("binding the socket");
    socket.listen(128).expect("marking the socket as listening");
    socket.into()
}

const GATE_SENDER_NUMBER: &str = "+2370000";
const GATE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const GATE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Every `SmsProvider::submit` this suite's own `dispatch::tick` calls,
/// including a doomed one against a killed Orange, must complete inside
/// one `tokio::test`'s own runtime without the test hanging — a connect
/// refusal is near-instant at the OS level (an RST, not a black hole), so
/// this budget is generous headroom, not a value this suite depends on
/// being tight.
fn orange_config(base_url: String) -> OrangeCmConfig {
    OrangeCmConfig {
        client_id: "kill-orange-gate-client".to_owned(),
        client_secret: "kill-orange-gate-secret".to_owned(),
        sender_number: GATE_SENDER_NUMBER.to_owned(),
        base_url,
        dlr_notify_url: None,
        connect_timeout: GATE_CONNECT_TIMEOUT,
        request_timeout: GATE_REQUEST_TIMEOUT,
    }
}

async fn mount_mtn_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = request
                .body_json()
                .unwrap_or_else(|_| serde_json::json!({}));
            let reference = body
                .get("reference")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "messageId": format!("mtn-{reference}"),
                "status": "Sent",
            }))
        })
        .mount(server)
        .await;
}

/// Everything the gate's own dispatch loop needs: the real Orange adapter
/// (pointed at a not-yet-listening address — [`orange_addr_str`] is bound
/// and released by the caller before this runs, see the test's own doc),
/// the real MTN adapter against a real, healthy `wiremock` server, and the
/// `WorkerContext` wiring both into the registry `dispatch::resolve_provider`
/// looks adapters up in. Split out for the same `clippy::too_many_lines`
/// reason every other helper in this file was.
struct GateHarness {
    ctx: WorkerContext,
    mtn_server: MockServer,
}

async fn build_gate_harness(
    db: &Cratestack,
    fixture: &GateFixture,
    orange_addr_str: &str,
) -> GateHarness {
    let orange_provider: Arc<dyn SmsProvider> = Arc::new(OrangeCmProvider::new(orange_config(
        format!("http://{orange_addr_str}"),
    )));

    let mtn_server = MockServer::start().await;
    mount_mtn_ok(&mtn_server).await;
    let mtn_provider: Arc<dyn SmsProvider> =
        Arc::new(MtnAggregatorProvider::new(MtnAggregatorConfig {
            api_key: "kill-orange-gate-mtn-key".to_owned(),
            sender_id: "VYMALO".to_owned(),
            base_url: mtn_server.uri(),
            tps_ceiling: 20.0,
            cost_per_segment_xaf: rust_decimal::Decimal::new(12, 0),
            supports_alphanumeric_sender: true,
            connect_timeout: GATE_CONNECT_TIMEOUT,
            request_timeout: GATE_REQUEST_TIMEOUT,
        }));

    let ctx = WorkerContext {
        db: db.clone(),
        providers: Arc::new(HashMap::from([
            (fixture.orange_key.clone(), orange_provider),
            (fixture.mtn_key.clone(), mtn_provider),
        ])),
    };
    GateHarness { ctx, mtn_server }
}

/// Phase 0: Orange is healthy — sanity, before this gate kills it. Never a
/// DLR here — every `SubmitDecision` this suite uses is `accepted()`,
/// whose `dlr_plan` is always empty (see `sms_fake_orange::fault::
/// SubmitDecision::accepted`'s own doc), so the DLR endpoint this fake is
/// configured with is never actually dialed. A syntactically valid but
/// unreachable URL is deliberate, matching `chaos_live_postgres.rs`'s own
/// `orange_config("http://127.0.0.1:1")` placeholder for a
/// parse_dlr-only provider instance. Returns the started `FakeOrange` —
/// the caller kills it (by dropping it) once this phase's own assertions
/// pass.
async fn run_baseline_phase(
    db: &Cratestack,
    sys: &CoolContext,
    ctx: &WorkerContext,
    app_id: &str,
    fixture: &GateFixture,
    orange_addr_str: &str,
) -> FakeOrange {
    let fake_orange = FakeOrange::start_on(
        reuseaddr_listener(orange_addr_str),
        FaultPolicy::always(SubmitDecision::accepted()),
        TokenPolicy::Always,
        "http://127.0.0.1:1/dlr/orange_cm",
        GATE_SENDER_NUMBER,
    )
    .await;

    let baseline = seed_message(db, app_id, OperatorCode::orange).await;
    tick(ctx, sys, "gate-worker").await.expect("tick"); // accepted -> queued (routed to Orange)
    tick(ctx, sys, "gate-worker").await.expect("tick"); // queued -> routed -> submitted (via the real, healthy Orange)

    let baseline_after = reload_message(db, &baseline.id).await;
    assert_eq!(
        baseline_after.state,
        MessageState::submitted,
        "sanity: Orange must be genuinely healthy before this gate kills it"
    );
    assert_eq!(baseline_after.providerId, Some(fixture.orange_id.clone()));
    assert_eq!(
        fake_orange.ledger().submit_count(&baseline.id),
        1,
        "sanity: the baseline message reached the real fake Orange exactly once"
    );
    fake_orange
}

/// Phase 1: the outage. Five Orange-destined messages (enough to cross the
/// five-consecutive-`Unavailable` threshold and open the circuit, §6.3)
/// and three MTN-destined messages, seeded together so both traffic
/// classes compete for the same dispatch ticks — the strongest available
/// proof that one doesn't starve the other. Proves three of #65's four
/// clauses, labelled inline; `orange_ledger` must already reflect the
/// baseline submission (its own `submits().len()` is checked against `1`,
/// not `0`).
async fn run_outage_phase(
    db: &Cratestack,
    sys: &CoolContext,
    ctx: &WorkerContext,
    app_id: &str,
    fixture: &GateFixture,
    orange_ledger: &sms_fake_orange::Ledger,
    mtn_server: &MockServer,
) -> (Vec<String>, Vec<String>) {
    let mut orange_bound_ids = Vec::new();
    for _ in 0..5 {
        orange_bound_ids.push(seed_message(db, app_id, OperatorCode::orange).await.id);
    }
    let mut mtn_bound_ids = Vec::new();
    for _ in 0..3 {
        mtn_bound_ids.push(seed_message(db, app_id, OperatorCode::mtn).await.id);
    }

    tick(ctx, sys, "gate-worker").await.expect("tick"); // accepted -> queued for all eight (routing: Orange still looks healthy at this instant)
    tick(ctx, sys, "gate-worker").await.expect("tick"); // queued -> routed -> submit: MTN succeeds; Orange connection-refused -> failover -> queued(MTN)
    tick(ctx, sys, "gate-worker").await.expect("tick"); // the five failed-over messages: queued -> routed -> submitted (via MTN)

    // Clause: "Assert MTN traffic is unaffected" — it must keep reaching
    // `submitted`, through the MTN provider, while Orange is down.
    for id in &mtn_bound_ids {
        let after = reload_message(db, id).await;
        assert_eq!(
            after.state,
            MessageState::submitted,
            "MTN-destined message {id} must reach submitted while Orange is down"
        );
        assert_eq!(after.providerId, Some(fixture.mtn_id.clone()));
    }

    // Clause: "Orange-destined traffic fails over cleanly" — reaches
    // submitted, and via the *alternate* route/provider specifically, not
    // merely "eventually succeeded".
    for id in &orange_bound_ids {
        let after = reload_message(db, id).await;
        assert_eq!(
            after.state,
            MessageState::submitted,
            "Orange-destined message {id} must still reach submitted via failover"
        );
        assert_eq!(
            after.providerId,
            Some(fixture.mtn_id.clone()),
            "must have failed over to the MTN route, not merely succeeded somehow"
        );
        assert!(
            after
                .stateReason
                .as_deref()
                .is_some_and(|r| r.contains("failover")),
            "stateReason should explain the reroute, got {:?}",
            after.stateReason
        );
    }

    // Clause: "nothing double-sends" — checked against the providers' own
    // request logs, not this system's Message rows.
    assert_eq!(
        orange_ledger.submits().len(),
        1,
        "Orange's own ledger must show no new entries during the whole outage — every \
         attempted connection failed at the transport level before ever reaching Orange's \
         request handler, so there is nothing there to have been duplicated"
    );
    let mtn_requests_during_outage = mtn_server
        .received_requests()
        .await
        .expect("wiremock tracks every request it received");
    assert_eq!(
        mtn_requests_during_outage.len(),
        mtn_bound_ids.len() + orange_bound_ids.len(),
        "MTN must have received exactly one submit per message that reached it — three native, \
         five failed-over, eight total, none duplicated"
    );

    // Clause (half): the circuit breaker opens on sustained failure.
    let orange_row_open = reload_provider(db, &fixture.orange_id).await;
    assert!(
        orange_row_open
            .circuitOpenUntil
            .is_some_and(|until| until > Utc::now()),
        "five consecutive connection-refused failures must open Orange's circuit breaker"
    );
    assert_eq!(
        orange_row_open.consecutiveFailures, 0,
        "opening the circuit resets the counter (dispatch.rs's own record_provider_failure)"
    );

    (orange_bound_ids, mtn_bound_ids)
}

/// Phase 2: recovery. Fresh Orange-destined messages must route straight
/// back to Orange (not via a failover reroute) and reach `submitted`
/// through the real, revived [`FakeOrange`] — proving the other half of
/// #65's circuit-breaker clause: it doesn't just open, it closes again.
async fn run_recovery_phase(
    db: &Cratestack,
    sys: &CoolContext,
    ctx: &WorkerContext,
    app_id: &str,
    fixture: &GateFixture,
    orange_ledger: &sms_fake_orange::Ledger,
) {
    let mut recovery_ids = Vec::new();
    for _ in 0..3 {
        recovery_ids.push(seed_message(db, app_id, OperatorCode::orange).await.id);
    }
    tick(ctx, sys, "gate-worker").await.expect("tick"); // accepted -> queued (routing sees the circuit closed again, picks Orange)
    tick(ctx, sys, "gate-worker").await.expect("tick"); // queued -> routed -> submitted (via the revived Orange)

    for id in &recovery_ids {
        let after = reload_message(db, id).await;
        assert_eq!(
            after.state,
            MessageState::submitted,
            "a recovery message must reach submitted once Orange is healthy again"
        );
        assert_eq!(
            after.providerId,
            Some(fixture.orange_id.clone()),
            "must have routed straight back to Orange, not stayed on MTN"
        );
        assert_eq!(
            orange_ledger.submit_count(id),
            1,
            "the revived Orange's own ledger must show exactly one submission for {id} — not \
             zero (never reached) and not two (double-sent)"
        );
    }
    assert_eq!(
        orange_ledger.submits().len(),
        recovery_ids.len(),
        "the revived Orange must have received exactly one request per recovery message, \
         nothing more"
    );
}

/// Forces the circuit's 60s cool-down forward, the same "force it for a
/// faster test" technique this codebase already uses elsewhere (e.g. the
/// handset-gate runbook's own `UPDATE messages SET lease_until = ...`) —
/// through a real delegate call, not raw SQL (R1). This exercises exactly
/// the same comparison (`circuitOpenUntil > now`,
/// `backends/crates/sms-worker/src/routing.rs::convert_provider`) that 60 real
/// seconds elapsing would, without waiting them out.
async fn force_circuit_cooldown_past(db: &Cratestack, sys: &CoolContext, provider_id: &str) {
    let row = reload_provider(db, provider_id).await;
    db.provider()
        .update(provider_id.to_owned())
        .set(schema::UpdateProviderInput {
            circuitOpenUntil: Some(Some(Utc::now() - ChronoDuration::seconds(1))),
            ..Default::default()
        })
        .if_match(row.version)
        .run(sys)
        .await
        .expect("forcing the circuit's cool-down into the past");
}

/// The whole gate, one realistic timeline: Orange healthy, Orange killed,
/// Orange revived. Each phase's assertions are labelled with which of
/// #65's four clauses they prove — see [`run_baseline_phase`],
/// [`run_outage_phase`], and [`run_recovery_phase`]'s own docs.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn orange_outage_fails_over_mtn_stays_up_nothing_double_sends_and_the_breaker_reopens() {
    let _guard = TEST_MUTEX.lock().await;
    let db = isolated_db().await;
    let fixture = seed_gate_fixture(&db).await;
    let app_id = seed_app(&db).await;
    let sys = sys();

    // A stable local address for "Orange" across its whole kill/revive
    // lifecycle — bound once, up front, then only ever the *listener*
    // bound to it changes. `OrangeCmProvider::base_url` is fixed at
    // construction, exactly like every other adapter in this workspace, so
    // this is what lets one adapter instance live through the entire test
    // (built inside `build_gate_harness`, once, below).
    let orange_probe = reuseaddr_listener("127.0.0.1:0");
    let orange_addr_str = orange_probe
        .local_addr()
        .expect("reading the bound address")
        .to_string();
    drop(orange_probe);

    let harness = build_gate_harness(&db, &fixture, &orange_addr_str).await;

    let fake_orange =
        run_baseline_phase(&db, &sys, &harness.ctx, &app_id, &fixture, &orange_addr_str).await;
    let orange_ledger_before_kill = fake_orange.ledger();

    // Kill Orange: drop the real, running FakeOrange. Every submit attempt
    // from this point on gets a genuine OS-level connection refusal — the
    // same mechanism `sms-provider-orange-cm`'s own
    // `a_connect_refusal_is_still_unavailable` proves in isolation,
    // exercised here through the real dispatch loop instead.
    drop(fake_orange);

    run_outage_phase(
        &db,
        &sys,
        &harness.ctx,
        &app_id,
        &fixture,
        &orange_ledger_before_kill,
        &harness.mtn_server,
    )
    .await;

    // Revive Orange: a second, independent FakeOrange bound to the exact
    // same address the first one used (see `reuseaddr_listener`'s own
    // doc). The already-constructed adapter inside `harness.ctx` needs no
    // change at all — its `base_url` was always this address.
    let fake_orange_revived = FakeOrange::start_on(
        reuseaddr_listener(&orange_addr_str),
        FaultPolicy::always(SubmitDecision::accepted()),
        TokenPolicy::Always,
        "http://127.0.0.1:1/dlr/orange_cm",
        GATE_SENDER_NUMBER,
    )
    .await;
    let orange_ledger_after_revival = fake_orange_revived.ledger();

    force_circuit_cooldown_past(&db, &sys, &fixture.orange_id).await;

    run_recovery_phase(
        &db,
        &sys,
        &harness.ctx,
        &app_id,
        &fixture,
        &orange_ledger_after_revival,
    )
    .await;
}
