//! `simulateRoute` (#54) against a real, fully migrated Postgres — the
//! actual `ProcedureRegistry` trait method, not `route_simulator.rs`'s
//! private helpers called directly. Same "call the trait method, not a
//! crate-private helper" discipline `requeue_job_live_postgres.rs`/
//! `replay_webhook_attempt_live_postgres.rs` already document in their own
//! module docs — this is what proves the whole chain (Layer 2's
//! `require_permission`, the real `Route`/`Provider` fetch under `sys()`,
//! operator classification against the real seeded `OperatorPrefixRule`
//! table, and `sms_routing::select_route` itself) end to end, not just
//! `route_simulator.rs`'s own DB-free unit tests (which already prove the
//! *rendering* step can't drift from the engine — see that module's own
//! `the_wire_result_matches_the_engines_own_decision`).
//!
//! Calling the trait method directly bypasses Layer 1 (`@allow`) — enforced
//! by the generated router wrapping this method, not by the method itself.
//! What *is* exercised here: Layer 2's `require_permission(ctx,
//! "route:read")` gate, the real fetch-and-decide path, and the
//! `noRoutesConfigured` distinction #62/#54 both call out explicitly.
//!
//! ```bash
//! cargo test -p sms-api --test simulate_route_live_postgres -- --ignored
//! ```

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, CoolError, FilterExpr, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, procedures::ProcedureRegistry, procedures::simulate_route, route,
};
use sms_api::{HashPepper, Procedures};

/// #102: this binary's own tests can race on Postgres's own `pg_type`
/// catalog the first time two of them prepare the exact same not-yet-cached
/// query shape at the same instant — see `backends/crates/sms-worker/tests/
/// claim_live_postgres.rs`'s own `TEST_MUTEX` doc for the full reasoning.
/// Load-bearing here for a second reason, the same one `dispatch_live_postgres.rs`
/// already documents for `Route`: every test in this file reads *every*
/// `Route` row in the database (`simulateRoute` has no `appId` filter to
/// scope by), so two tests running interleaved would see each other's
/// fixtures. Sequential execution under this mutex, plus scoping every
/// fixture route's `matchAppId` to that test's own unique app id (never
/// `None`/wildcard), is what keeps one test's routes invisible to another
/// test's candidate even though nothing deletes them between tests.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

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

/// `Route`/`Provider` writes need a human role — `owner` is the loosest of
/// either model's admitted roles, same reasoning `if_match_live_postgres.rs`'s
/// own `owner()` gives.
fn owner() -> CoolContext {
    Principal {
        sub: "simulate-route-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// The context the admin console's own machine credential produces in
/// production once provisioned with the `route:read` scope (#54) —
/// `kind == "app"`, matching `Route`/`Provider`'s own `@@allow` (this PR),
/// plus the Layer 2 scope `simulate_route`'s `require_permission` checks.
fn app_caller_with_route_read() -> CoolContext {
    let mut ctx = Principal {
        sub: "simulate-route-test-console-client".to_owned(),
        kind: PrincipalKind::App,
        role: "app".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions.insert(
        "scope".to_owned(),
        Value::String("sms:send route:read".to_owned()),
    );
    ctx
}

/// The identical caller shape, but without the `route:read` scope — the
/// exact "an omitted scope yields denial" shape §5.2 documents.
fn app_caller_without_route_read() -> CoolContext {
    let mut ctx = Principal {
        sub: "simulate-route-test-console-client-no-scope".to_owned(),
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
    HashPepper::new("simulate-route-live-postgres-test-pepper-well-over-the-minimum-length")
        .expect("test pepper meets HashPepper::new's minimum length")
}

async fn create_active_provider(db: &Cratestack, suffix: &str) -> String {
    let created = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!("simroute_{suffix}"),
            displayName: "Simulate Route Test Provider".to_owned(),
            kind: schema::ProviderKind::aggregator_http,
            config: "{}".to_owned(),
            credentialRef: "vault://test".to_owned(),
            maxTps: 5.0,
            maxDailySubmissions: 1000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: "15".parse().expect("15 parses as a Decimal"),
            healthCheckedAt: None,
            // #63 added the circuit breaker's own columns.
            // `consecutiveFailures` carries a @default so it stays out of the
            // create input; `circuitOpenUntil` does not, so every fixture that
            // builds a Provider has to name it.
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("creating the test Provider");

    db.provider()
        .update(created.id.clone())
        .set(schema::UpdateProviderInput {
            state: Some(schema::ProviderState::active),
            ..Default::default()
        })
        // #59: Provider is @version'd.
        .if_match(created.version)
        .run(&owner())
        .await
        .expect("activating the test Provider");

    created.id
}

/// A `Route` scoped to `app_id` via `matchAppId` — never a wildcard — so
/// this test's fixture is invisible to every other test's own candidate.
/// See this file's own module doc on `TEST_MUTEX` for why that matters.
async fn create_route(
    db: &Cratestack,
    provider_id: &str,
    app_id: &str,
    priority: i64,
    weight: i64,
) -> schema::Route {
    db.route()
        .create(schema::CreateRouteInput {
            name: format!("simulate-route-test-{}", unique_suffix()),
            priority,
            weight,
            enabled: true,
            matchOperator: None,
            matchClass: None,
            matchAppId: Some(app_id.to_owned()),
            matchPrefix: None,
            providerId: provider_id.to_owned(),
            failoverRouteId: None,
        })
        .run(&owner())
        .await
        .expect("creating the test Route")
}

/// The headline case: this test's own route wins for its own candidate, is
/// reported `eligible` with `winningBand: true`, and `noRoutesConfigured`
/// is `false`.
///
/// **Found live, not assumed**: an earlier draft of this test asserted
/// `result.evaluations.len() == 1`, reasoning that `matchAppId`-scoping a
/// route makes it "invisible" to another candidate. That's wrong —
/// `Decision.evaluations` (and therefore `SimulateRouteResult.evaluations`)
/// covers *every* `Route` row in the table unconditionally, exactly as
/// documented (`sms_routing::Decision`'s own doc: "so a caller never has to
/// re-derive 'why wasn't route X picked' from anything but this list").
/// `matchAppId` only changes a route's own *outcome*
/// (`eligible`/`predicate_failed`), never whether it appears at all. Running
/// this suite's tests together (rather than only this file's own
/// DB-free unit tests) surfaced the gap: another test's leftover route was
/// present in `evaluations` too, correctly reported `predicate_failed`, and
/// the hardcoded `== 1` failed for a reason that had nothing to do with
/// production code. Fixed by finding this test's own route by id, which is
/// what the test actually needs to prove and is correct regardless of how
/// many other routes the shared database happens to hold.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_single_matching_route_wins_and_is_reported_eligible() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let app_id = format!("simroute-app-{suffix}");

    let provider_id = create_active_provider(&db, &suffix).await;
    let route = create_route(&db, &provider_id, &app_id, 0, 1).await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — the "sanctioned way to invoke a procedure from
    // non-HTTP code" per that function's own doc comment.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_route_read();
    let args = simulate_route::Args {
        args: schema::SimulateRouteInput {
            msisdn: "+237677123456".to_owned(),
            class: schema::MessageClass::otp,
            appId: app_id,
            draw: None,
        },
    };
    let result = simulate_route::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.simulate_route(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("simulateRoute must succeed for a caller with route:read");

    assert!(!result.noRoutesConfigured);
    let own_evaluation = result
        .evaluations
        .iter()
        .find(|evaluation| evaluation.routeId == route.id)
        .expect("this test's own route must appear in evaluations");
    assert_eq!(own_evaluation.outcome, schema::RouteOutcomeKind::eligible);
    assert!(own_evaluation.winningBand);
    let winner = result.winner.expect("this candidate's own route must win");
    assert_eq!(winner.routeId, route.id);
    assert_eq!(winner.providerId, provider_id);
}

/// A route whose `matchAppId` disagrees with the candidate is reported
/// `predicate_failed` with the mismatch spelled out, and nothing wins.
///
/// Finds its own route by id among `result.evaluations` rather than
/// asserting a fixed length — see `a_single_matching_route_wins_and_is_reported_eligible`'s
/// own doc for why: `evaluations` covers every `Route` row in the table,
/// including other tests' leftover fixtures, not just this candidate's own.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_route_scoped_to_a_different_app_is_reported_as_a_predicate_failure() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let route_app_id = format!("simroute-app-{suffix}-owner");
    let candidate_app_id = format!("simroute-app-{suffix}-caller");

    let provider_id = create_active_provider(&db, &suffix).await;
    let route = create_route(&db, &provider_id, &route_app_id, 0, 1).await;

    // cratestack 0.7.13 (cratestack#512): see the identical comment on the
    // test above.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_route_read();
    let args = simulate_route::Args {
        args: schema::SimulateRouteInput {
            msisdn: "+237677123456".to_owned(),
            class: schema::MessageClass::otp,
            appId: candidate_app_id.clone(),
            draw: None,
        },
    };
    let result = simulate_route::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.simulate_route(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("simulateRoute must succeed for a caller with route:read");

    assert!(result.winner.is_none());
    let evaluation = result
        .evaluations
        .iter()
        .find(|evaluation| evaluation.routeId == route.id)
        .expect("this test's own route must appear in evaluations");
    assert_eq!(
        evaluation.outcome,
        schema::RouteOutcomeKind::predicate_failed
    );
    assert_eq!(
        evaluation.predicateKind,
        Some(schema::PredicateKind::app_id)
    );
    assert_eq!(
        evaluation.predicateExpected.as_deref(),
        Some(route_app_id.as_str())
    );
    assert_eq!(
        evaluation.predicateActual.as_deref(),
        Some(candidate_app_id.as_str())
    );
}

/// The #62/#54-documented distinct state: zero `Route` rows in the whole
/// system at all, not just zero eligible for this candidate. Deletes every
/// row first (rather than relying on execution order to find an empty
/// table) — see this file's own module doc on why that's safe regardless of
/// which order the suite's tests happen to run in.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn zero_configured_routes_is_reported_distinctly_from_zero_eligible_routes() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let existing = db
        .route()
        .find_many()
        .run(&owner())
        .await
        .expect("listing every Route row to clear them");
    for row in existing {
        // cratestack 0.7.13 (cratestack#519): DELETE on an `@version` model
        // now enforces `If-Match` — `row.version` is the value this exact
        // `find_many` call just read, so it always matches.
        let version = row.version;
        db.route()
            .delete(row.id)
            .if_match(version)
            .run(&owner())
            .await
            .expect("deleting a leftover Route row");
    }

    // cratestack 0.7.13 (cratestack#512): see the identical comment above.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_route_read();
    let args = simulate_route::Args {
        args: schema::SimulateRouteInput {
            msisdn: "+237677123456".to_owned(),
            class: schema::MessageClass::otp,
            appId: format!("simroute-app-{}", unique_suffix()),
            draw: None,
        },
    };
    let result = simulate_route::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.simulate_route(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("simulateRoute must succeed against an empty Route table");

    assert!(result.noRoutesConfigured);
    assert!(result.evaluations.is_empty());
    assert!(result.winner.is_none());
}

/// Layer 2 (§5.1): an app-kind caller with no `route:read` scope is denied
/// before any `Route`/`Provider` row is ever read.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn simulate_route_denies_a_caller_with_no_route_read_scope() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`, which runs the real Layer 1 `@allow` check first —
    // `auth().kind == "app"` already admits this caller there
    // (`schema.cstack`'s `simulateRoute` `@allow`), and `simulateRoute`
    // carries no `@authorize` model check, so this stays a genuine Layer 2
    // (`require_permission`) denial, not a Layer 1 one.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_without_route_read();
    let args = simulate_route::Args {
        args: schema::SimulateRouteInput {
            msisdn: "+237677123456".to_owned(),
            class: schema::MessageClass::otp,
            appId: format!("simroute-app-{}", unique_suffix()),
            draw: None,
        },
    };
    let error = simulate_route::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.simulate_route(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect_err("a caller with no route:read scope must be denied");

    assert!(
        matches!(error, CoolError::Forbidden(_)),
        "expected Forbidden, got {error:?}"
    );
    if let CoolError::Forbidden(message) = error {
        assert!(
            message.contains("route:read"),
            "expected the denial to name the missing permission: {message}"
        );
    }
}

/// A supplied `draw` is honoured exactly — the same value handed to
/// `simulateRoute` twice in a row must produce the same winner in a tied
/// weighted band, proving the injected-draw property #54's own brief
/// describes ("precisely so a simulator can replay a decision
/// deterministically") holds through this procedure, not just through
/// `sms_routing::select_route` in isolation.
///
/// **Found live, not assumed**: the first draft of this test hardcoded
/// "the heavier-weighted route always wins at draw=0.9", reasoning from
/// creation order ("light" created first, so it must get the low end of
/// the cumulative-weight range). That is wrong — `sms_routing::select_route`
/// bands members in *id-ascending* order (`fetch_routes_and_providers`'s
/// own `.order_by(route::id().asc())`, matching production), and `Cuid`
/// generation is not correlated with creation order, confirmed by actually
/// running this test repeatedly: which of the two routes sorts first by id
/// (and therefore which one owns the low vs. high share of `[0.0, 1.0)`)
/// varied from run to run, while the *replayed* winner for a given run
/// never did. So the real, order-independent property this test proves is
/// reproducibility plus draw-sensitivity — computed from what the engine
/// itself reports (`first.tieBreak`), never a hardcoded assumption about
/// which named route that turns out to be.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_supplied_draw_reproduces_the_same_winner_on_replay() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let app_id = format!("simroute-app-{suffix}");

    let provider_id = create_active_provider(&db, &suffix).await;
    // Two same-priority, differently-weighted routes so a draw actually has
    // a tie to break — which one ends up with the wider share of
    // `[0.0, 1.0)` depends on id ordering, not on which is created first
    // (see this test's own doc above), so nothing below assumes it.
    let route_a = create_route(&db, &provider_id, &app_id, 0, 1).await;
    let route_b = create_route(&db, &provider_id, &app_id, 0, 3).await;

    let input = |draw: f64| simulate_route::Args {
        args: schema::SimulateRouteInput {
            msisdn: "+237677123456".to_owned(),
            class: schema::MessageClass::otp,
            appId: app_id.clone(),
            draw: Some(draw),
        },
    };

    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_route_read();

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`.
    let first_args = input(0.9);
    let first = simulate_route::invoke_with_db(&db, &first_args, &ctx, |authorized| {
        procedures.simulate_route(&db, &ctx, first_args.clone(), authorized)
    })
    .await
    .expect("first simulateRoute call");
    let second_args = input(0.9);
    let second = simulate_route::invoke_with_db(&db, &second_args, &ctx, |authorized| {
        procedures.simulate_route(&db, &ctx, second_args.clone(), authorized)
    })
    .await
    .expect("second simulateRoute call with the identical draw");

    assert_eq!(
        first.winner.as_ref().map(|w| w.routeId.clone()),
        second.winner.as_ref().map(|w| w.routeId.clone()),
        "the identical draw against the identical route set must reproduce the identical winner"
    );

    let winner_id = first
        .winner
        .as_ref()
        .expect("a two-member band always has a winner")
        .routeId
        .clone();
    assert!(
        winner_id == route_a.id || winner_id == route_b.id,
        "the winner must be one of this test's own two fixture routes, not something else"
    );

    // Draw-sensitivity: read the engine's own reported ranges (never
    // re-derived by this test) and pick a draw that lands in whichever
    // range did NOT win above, then confirm it wins instead — proving the
    // draw genuinely steers the outcome rather than one route always
    // winning regardless of what's passed in.
    let tie_break = first.tieBreak.expect("a two-member band always ties");
    let other_range = tie_break
        .ranges
        .iter()
        .find(|range| range.routeId != winner_id)
        .expect("a two-member band has exactly one other range");
    // Comfortably inside `[low, high)`, away from either boundary.
    let other_draw = other_range.low + (other_range.high - other_range.low) / 2.0;

    let third_args = input(other_draw);
    let third = simulate_route::invoke_with_db(&db, &third_args, &ctx, |authorized| {
        procedures.simulate_route(&db, &ctx, third_args.clone(), authorized)
    })
    .await
    .expect("third simulateRoute call with a draw landing in the other route's range");
    assert_eq!(
        third.winner.as_ref().map(|w| w.routeId.clone()),
        Some(other_range.routeId.clone()),
        "a draw landing in the other route's own reported range must make that route win"
    );
    assert_ne!(
        third.winner.map(|w| w.routeId),
        Some(winner_id),
        "the two draws must actually resolve the tie differently, not agree by coincidence"
    );
}

/// Sanity check that this file's own `matchAppId`-scoping discipline
/// actually isolates fixtures from each other — read straight from the
/// database rather than assumed, since a bug here would make every other
/// test in this file's counts silently wrong on a reordered run.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn fixture_routes_from_other_tests_do_not_leak_into_an_unrelated_candidate() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let app_id = format!("simroute-app-{suffix}-isolated");

    let count_before = db
        .route()
        .find_many()
        .where_expr(FilterExpr::from(route::matchAppId().eq(app_id.clone())))
        .run(&owner())
        .await
        .expect("counting routes scoped to a brand-new app id")
        .len();
    assert_eq!(
        count_before, 0,
        "a freshly minted app id must start with zero scoped routes, regardless of what earlier tests left behind"
    );

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`.
    let procedures = Procedures::new(test_pepper());
    let ctx = app_caller_with_route_read();
    let args = simulate_route::Args {
        args: schema::SimulateRouteInput {
            msisdn: "+237677123456".to_owned(),
            class: schema::MessageClass::otp,
            appId: app_id,
            draw: None,
        },
    };
    let result = simulate_route::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.simulate_route(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("simulateRoute must succeed");

    // Every other test's own fixture routes are scoped to their own app
    // id, so none of them can appear as `eligible`/`predicate_failed`
    // evaluations here even though the table itself isn't empty — the
    // engine still evaluates every row (that's `noRoutesConfigured`'s own
    // job to distinguish), but only via a real predicate failure.
    for evaluation in &result.evaluations {
        assert_eq!(
            evaluation.outcome,
            schema::RouteOutcomeKind::predicate_failed,
            "a route belonging to another test must fail this candidate's app_id predicate, not sit eligible"
        );
    }
    assert!(result.winner.is_none());
}
