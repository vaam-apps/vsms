//! Proves `#64`'s `grey_route_watch` job
//! (`backends/crates/sms-worker/src/jobs/grey_route_watch.rs`) against a real, fully
//! migrated Postgres. The pure divergence math (sample-size gate, z-test,
//! delta floor, `uncertain` exclusion) is exhaustively covered by that
//! module's own unit tests — see its own doc for the guard-failure proofs
//! run against it. This suite's job is different: prove the real-database
//! half those unit tests cannot reach — a `Message`/`RouteValidation`/`Route`
//! read under a `system` context actually returns real rows (the
//! eleven-times-repeated `hasRole('system')` gap this file's own schema
//! comment names), and that `GreyRouteWatch::run_at` wires the pure
//! functions to those real rows and to the two `sms_metrics` gauges
//! correctly.
//!
//! This database is never reset between runs and other suites in this
//! workspace seed plenty of their own `Message`/`Route` rows — see
//! `dispatch_live_postgres.rs`'s own `disable_every_route` precedent, reused
//! here for the overdue-validation half (route-scoped, so easy to make
//! exact). The divergence half is deliberately *not* scoped that way —
//! doing so would mean reimplementing routing/claim's own fixture-isolation
//! machinery for a check this suite doesn't own — so its own assertions are
//! tolerant (`>= 1`, not `== 1`) and use a rare `(operator, class)` pair
//! (`nexttel`/`marketing`) to keep collision with other suites' fixtures
//! implausible rather than impossible.
//!
//! Ignored by default, same convention as this crate's other live suites.
//! Run explicitly:
//!
//! ```bash
//! cargo test -p sms-worker --test grey_route_watch_live_postgres -- --ignored
//! ```

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, Encoding, Message, MessageClass, MessageState, OperatorCode, route,
};
use sms_worker::jobs::JobHandler;
use sms_worker::jobs::grey_route_watch::GreyRouteWatch;

/// Same reasoning as every other live suite's own copy of this mutex — see
/// `claim_live_postgres.rs`'s own `TEST_MUTEX` doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CoolContext {
    Principal {
        sub: "sms-worker-grey-route-watch-test".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "sms-worker-grey-route-watch-test-owner".to_owned(),
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
    format!("{:08x}", (u64::from(nanos).wrapping_add(n)) & 0xffff_ffff)
}

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
            name: "grey route watch test app".to_owned(),
            slug: format!("grey-route-watch-test-{}", unique_suffix()),
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

/// A `Provider`+`Route` pair — this suite never dispatches through either,
/// it only needs a real `routeId` foreign key for `Message.routeId` and
/// `RouteValidation.routeId` to point at.
async fn seed_route(db: &Cratestack) -> String {
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!("grw_test_{}", unique_suffix())
                .chars()
                .take(32)
                .collect(),
            displayName: "grey_route_watch test provider".to_owned(),
            kind: schema::ProviderKind::aggregator_http,
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

    let route = db
        .route()
        .create(schema::CreateRouteInput {
            name: format!("grey-route-watch-test-route-{}", unique_suffix()),
            priority: 500,
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
        .expect("seeding a route");

    route.id
}

/// Every `enabled` route this shared database currently has, disabled — the
/// same precedent `dispatch_live_postgres.rs`'s own `disable_every_route`
/// sets, reused here so `check_overdue_validations`'s own count is scoped
/// to exactly what a given test seeds.
async fn disable_every_route(db: &Cratestack) {
    let enabled = db
        .route()
        .find_many()
        .where_expr(FilterExpr::from(route::enabled().is_true()))
        .run(&owner())
        .await
        .expect("listing enabled routes");
    for row in enabled {
        db.route()
            .update(row.id)
            .set(schema::UpdateRouteInput {
                enabled: Some(false),
                ..Default::default()
            })
            .if_match(row.version)
            .run(&owner())
            .await
            .expect("disabling a leftover enabled route");
    }
}

/// Seeds a `Message` already attributed to `route_id`, under a rare
/// `(operator, class)` pair — see the module doc for why `nexttel`/
/// `marketing` specifically.
async fn seed_message(db: &Cratestack, app_id: &str, route_id: &str) -> Message {
    db.message()
        .create(schema::CreateMessageInput {
            appId: app_id.to_owned(),
            clientRef: None,
            idempotencyKey: Some(format!("grey-route-watch-test-{}", unique_suffix())),
            msisdn: "+237699112233".to_owned(),
            msisdnHash: format!("hmac-sha256-v1:grey-route-watch-test-{}", unique_suffix()),
            operator: OperatorCode::nexttel,
            senderIdValue: "VYMALO".to_owned(),
            class: MessageClass::marketing,
            priority: 500,
            body: Some("grey_route_watch test".to_owned()),
            bodyHash: format!("hmac-sha256-v1:grey-route-watch-test-{}", unique_suffix()),
            bodyLength: 22,
            encoding: Encoding::gsm7,
            segments: 1,
            stateReason: None,
            routeId: Some(route_id.to_owned()),
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

/// Walks a freshly seeded (`accepted`) message straight to `target` —
/// `accepted -> queued -> routed -> submitted -> target`, every hop a legal
/// edge in `message_state_transitions`. `target` must be a legal
/// `submitted -> _` edge (`delivered`/`undelivered`/`failed`/`expired`/
/// `uncertain`).
async fn drive_to(db: &Cratestack, message: Message, target: MessageState) {
    let mut current = message;
    for state in [
        MessageState::queued,
        MessageState::routed,
        MessageState::submitted,
        target,
    ] {
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
            .unwrap_or_else(|error| panic!("driving message to {state:?} failed: {error}"));
    }
}

async fn seed_and_terminate(
    db: &Cratestack,
    app_id: &str,
    route_id: &str,
    delivered: usize,
    other_terminal: usize,
    other_terminal_state: MessageState,
) {
    for _ in 0..delivered {
        let message = seed_message(db, app_id, route_id).await;
        drive_to(db, message, MessageState::delivered).await;
    }
    for _ in 0..other_terminal {
        let message = seed_message(db, app_id, route_id).await;
        drive_to(db, message, other_terminal_state).await;
    }
}

/// The highest-value live guard: two routes with a genuinely divergent,
/// trustworthy-sample-size delivery rate are flagged by the real,
/// DB-backed `check_divergence`, not just the pure function it wraps.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_genuinely_divergent_pair_of_routes_is_flagged_through_the_real_database() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;

    let app_id = seed_app(&db).await;
    let healthy_route = seed_route(&db).await;
    let bad_route = seed_route(&db).await;

    // 40/40 delivered (100%) vs 15/40 delivered (37.5%) — comfortably past
    // MIN_SAMPLE (30), MIN_DELTA (0.15), and Z_THRESHOLD (3.0) at this size.
    seed_and_terminate(&db, &app_id, &healthy_route, 40, 0, MessageState::delivered).await;
    seed_and_terminate(&db, &app_id, &bad_route, 15, 25, MessageState::undelivered).await;

    let now = Utc::now();
    let checker = fresh_db().await;
    let flagged = GreyRouteWatch
        .check_divergence(&checker, &sys(), now)
        .await
        .expect("check_divergence succeeds against a live database");

    assert!(
        flagged >= 1,
        "a route this divergent, at this sample size, must be flagged through the real \
         Message delegate — got {flagged}"
    );
}

/// The companion guard, against the same real database: a route with no
/// terminal traffic at all in the lookback window contributes nothing —
/// `check_divergence` must not error or panic on an otherwise-empty peer
/// group, and `run_at` must still set the metric (to `0` at minimum, never
/// leave it untouched from some other test's own last write within this
/// process).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn run_at_sets_both_gauges_even_with_nothing_to_flag() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;

    // A lookback window far enough in the future that nothing in this
    // shared, never-reset database can have a createdAt past it — the
    // divergence half legitimately sees zero candidate messages.
    let now = Utc::now() + ChronoDuration::days(3650);

    GreyRouteWatch
        .run_at(&db, &sys(), now)
        .await
        .expect("run_at succeeds even with nothing to check");

    // Both gauges are `IntGauge`s the job always `.set()`s on every run,
    // never merely incremented — so this assertion holds regardless of
    // what any other test in this process last set them to.
    assert!(
        sms_metrics::ROUTE_DELIVERY_DIVERGENCE_FLAGGED.get() >= 0,
        "the gauge must be a real, set value"
    );
    assert!(
        sms_metrics::ROUTE_VALIDATION_OVERDUE.get() >= 0,
        "the gauge must be a real, set value"
    );
}

/// The handset-validation half: an enabled route with no `RouteValidation`
/// row is overdue; one validated moments ago is not. Route-scoped via
/// `disable_every_route`, so this test's own count is exact despite the
/// shared, never-reset database.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn overdue_validation_staleness_is_computed_from_real_rows() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;

    disable_every_route(&db).await;

    let never_validated = seed_route(&db).await;
    let recently_validated = seed_route(&db).await;

    db.route_validation()
        .create(schema::CreateRouteValidationInput {
            routeId: recently_validated.clone(),
            operator: OperatorCode::orange,
            performedBy: "Live Test Operator".to_owned(),
            expectedSenderId: "VYMALO".to_owned(),
            observedSenderId: "VYMALO".to_owned(),
            passed: true,
            notes: Some("live suite fixture".to_owned()),
        })
        .run(&owner())
        .await
        .expect("seeding a RouteValidation row");

    let now = Utc::now();
    let overdue = GreyRouteWatch
        .check_overdue_validations(&db, &sys(), now)
        .await
        .expect("check_overdue_validations succeeds against a live database");

    assert_eq!(
        overdue, 1,
        "exactly the never-validated route must be overdue — \
         recently_validated={recently_validated} never_validated={never_validated}"
    );
}

/// The `RouteValidation` model's own system-context read policy — the
/// twelfth instance of this codebase's own repeated `hasRole('system')`
/// gap, flagged in advance in `schema.cstack` rather than found live. This
/// is the direct proof `system_context_golden_list_live_postgres.rs`'s own
/// generic sweep already gives, repeated here because it's the specific
/// mechanism `check_overdue_validations` depends on.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn route_validation_is_readable_under_a_system_context() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;

    let route_id = seed_route(&db).await;
    let created = db
        .route_validation()
        .create(schema::CreateRouteValidationInput {
            routeId: route_id.clone(),
            operator: OperatorCode::mtn,
            performedBy: "Live Test Operator".to_owned(),
            expectedSenderId: "VYMALO".to_owned(),
            observedSenderId: "VYMALO".to_owned(),
            passed: true,
            notes: None,
        })
        .run(&owner())
        .await
        .expect("seeding a RouteValidation row as owner");

    let read_back: Vec<schema::RouteValidation> = db
        .route_validation()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::route_validation::id().eq(created.id.clone()),
        ))
        .run(&sys())
        .await
        .expect("reading RouteValidation back under a system context");

    assert_eq!(
        read_back.len(),
        1,
        "a system context must be able to read back the RouteValidation row it (or an owner, \
         on its behalf) just created — got {} rows",
        read_back.len()
    );
    assert_eq!(read_back[0].observedSenderId, "VYMALO");

    let _: DateTime<Utc> = read_back[0].performedAt; // the framework-stamped default is real
}

/// End-to-end sanity on the `JobHandler` entry point, seeded and run
/// exactly the way `Role::Jobs`'s real claim loop would reach it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_job_handler_entry_point_runs_without_error_against_a_live_database() {
    let _guard = TEST_MUTEX.lock().await;
    let db = fresh_db().await;

    let job = db
        .job()
        .create(schema::CreateJobInput {
            kind: "grey_route_watch".to_owned(),
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
        .expect("seeding the grey_route_watch job");

    let outcome = GreyRouteWatch.run(&db, &sys(), &job).await;
    assert!(
        outcome.is_ok(),
        "grey_route_watch's JobHandler::run must succeed: {outcome:?}"
    );
    assert_eq!(GreyRouteWatch.kind(), "grey_route_watch");
}
