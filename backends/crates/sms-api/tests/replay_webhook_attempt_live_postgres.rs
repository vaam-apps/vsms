//! `replayWebhookAttempt` (#43) against a real, fully migrated Postgres —
//! the actual `ProcedureRegistry` trait method, not a crate-private helper
//! called directly, the same discipline `rotate_webhook_secret_live_postgres.rs`
//! and `send_message_live_postgres.rs` both document in their own module
//! docs.
//!
//! Calling the trait method directly (rather than over real HTTP) bypasses
//! Layer 1 (`@allow`/`@authorize`) entirely — those are enforced by the
//! generated router wrapping this method, not by the method itself — the
//! same scoping `send_message_live_postgres.rs`'s own `app_caller` doc
//! already spells out. What *is* exercised here, because it's hand-written
//! inside `Procedures::replay_attempt`'s own body rather than generated:
//! the Layer 2 `require_permission(ctx, "webhook:manage")` gate, and every
//! bit of this procedure's actual state-machine and circuit-breaker logic.
//!
//! ```bash
//! cargo test -p sms-api --test replay_webhook_attempt_live_postgres -- --ignored
//! ```

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, CoolError, FilterExpr, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, procedures::replay_webhook_attempt, procedures::ProcedureRegistry, webhook_endpoint,
    AttemptState, Cratestack, CreateWebhookAttemptInput, CreateWebhookEndpointInput,
    UpdateWebhookAttemptInput, UpdateWebhookEndpointInput,
};
use sms_api::{HashPepper, Procedures};

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests — run concurrently by Rust's default multi-threaded test
/// harness — can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape at the
/// same instant. See `backends/crates/sms-worker/tests/claim_live_postgres.rs`'s
/// own `TEST_MUTEX` doc for the full reasoning — same mechanism, same fix.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> CoolContext {
    Principal {
        sub: "replay-webhook-attempt-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CoolContext {
    Principal {
        sub: "replay-webhook-attempt-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// A human caller carrying exactly the permission `replayWebhookAttempt`'s
/// own Layer 2 gate (`require_permission(ctx, "webhook:manage")`) checks
/// for — §5.2's own vocabulary, `developer`'s permission. This function
/// never goes through a real token issuance path (no human-login flow
/// exists in this deployment — see AGENTS.md's M1/#24 notes), so it has to
/// carry the claim by hand, the same way `send_message_live_postgres.rs`'s
/// own `app_caller` does for `scope`.
fn developer_with_webhook_manage() -> CoolContext {
    let mut ctx = Principal {
        sub: "replay-webhook-attempt-test-developer".to_owned(),
        kind: PrincipalKind::User,
        role: "developer".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions.insert(
        "perms".to_owned(),
        Value::List(vec![Value::String("webhook:manage".to_owned())]),
    );
    ctx
}

/// The same role, but with no `perms` claim at all — the exact "an omitted
/// scope yields denial" shape §5.2 documents, extended to `perms` by
/// `require_permission`'s own doc comment.
fn developer_without_permission() -> CoolContext {
    Principal {
        sub: "replay-webhook-attempt-test-developer-no-perms".to_owned(),
        kind: PrincipalKind::User,
        role: "developer".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn test_pepper() -> HashPepper {
    HashPepper::new("replay-webhook-attempt-live-postgres-test-pepper-well-over-minimum")
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
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

async fn seed_app(db: &Cratestack, suffix: &str) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "replay webhook attempt live test app".to_owned(),
            slug: format!("replay-webhook-attempt-test-{suffix}"),
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

async fn seed_endpoint(db: &Cratestack, suffix: &str, app_id: &str) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: "https://example.test/webhooks/vsms".to_owned(),
            eventTypes: " message.delivered ".to_owned(),
            secret: format!("whsec_test_{suffix}"),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding the webhook endpoint")
}

/// Seeds a `pending` attempt (the shape #38's subscribers produce) and
/// walks it through legal edges only (`attempt_state_transitions`, §2.10)
/// to whatever `target` state a test needs — mirroring
/// `hooks_live_postgres.rs`'s own `clear_claimable_backlog` walk, since
/// `pending`/`failed` have no direct edge to `dead`.
async fn seed_attempt_in_state(
    db: &Cratestack,
    endpoint_id: &str,
    aggregate_id: &str,
    target: AttemptState,
) -> schema::WebhookAttempt {
    let attempt = db
        .webhook_attempt()
        .create(CreateWebhookAttemptInput {
            endpointId: endpoint_id.to_owned(),
            sourceEventId: cratestack::uuid::Uuid::new_v4(),
            aggregateId: aggregate_id.to_owned(),
            eventType: "message.delivered".to_owned(),
            payload: r#"{"messageId":"placeholder"}"#.to_owned(),
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
        .expect("seeding a pending webhook attempt");

    if target == AttemptState::pending {
        return attempt;
    }

    let delivering = db
        .webhook_attempt()
        .update(attempt.id.clone())
        .set(UpdateWebhookAttemptInput {
            state: Some(AttemptState::delivering),
            attempts: Some(3),
            lastStatusCode: Some(Some(503)),
            lastError: Some(Some("simulated failure".to_owned())),
            leaseOwner: Some(Some("simulated-worker".to_owned())),
            ..Default::default()
        })
        .if_match(attempt.version)
        .run(&sys())
        .await
        .expect("moving the seeded attempt to delivering");

    if target == AttemptState::delivering {
        return delivering;
    }

    db.webhook_attempt()
        .update(delivering.id.clone())
        .set(UpdateWebhookAttemptInput {
            state: Some(target),
            ..Default::default()
        })
        .if_match(delivering.version)
        .run(&sys())
        .await
        .unwrap_or_else(|error| panic!("moving the seeded attempt to {target:?}: {error}"))
}

/// The headline case: replaying a `dead` attempt resets it to `pending`
/// with a fresh counter and clears the bookkeeping from the failed run
/// that killed it — while leaving `id`/`sourceEventId`/`endpointId`
/// untouched, which is what makes "same event id, same signature
/// semantics" true by construction rather than something this procedure
/// has to engineer (§8.5's own "Implementation, #43" note).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn replaying_a_dead_attempt_resets_it_to_pending_with_a_fresh_counter() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id).await;
    let seeded = seed_attempt_in_state(
        &db,
        &endpoint.id,
        "cmsgreplayxdead0000000",
        AttemptState::dead,
    )
    .await;
    assert_eq!(seeded.state, AttemptState::dead);
    assert_eq!(seeded.attempts, 3);

    let before = Utc::now();
    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — the "sanctioned way to invoke a procedure from
    // non-HTTP code" per that function's own doc comment.
    let procedures = Procedures::new(test_pepper());
    let ctx = developer_with_webhook_manage();
    let args = replay_webhook_attempt::Args {
        args: schema::ReplayWebhookAttemptInput {
            attemptId: seeded.id.clone(),
        },
    };
    let replayed = replay_webhook_attempt::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.replay_webhook_attempt(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("replaying a dead attempt must succeed");

    assert_eq!(replayed.id, seeded.id, "replay must reset the same row");
    assert_eq!(
        replayed.sourceEventId, seeded.sourceEventId,
        "the envelope's dedupe key (sourceEventId) must survive a replay unchanged"
    );
    assert_eq!(replayed.endpointId, endpoint.id);
    assert_eq!(replayed.state, AttemptState::pending);
    assert_eq!(
        replayed.attempts, 0,
        "replay must reset the attempts counter"
    );
    assert!(replayed.lastStatusCode.is_none());
    assert!(replayed.lastError.is_none());
    assert!(replayed.leaseOwner.is_none());
    assert!(replayed.leaseUntil.is_none());
    let next_attempt_at = replayed
        .nextAttemptAt
        .expect("a replayed attempt must be immediately due");
    assert!(
        next_attempt_at >= before,
        "nextAttemptAt should be stamped at replay time, not left stale"
    );
}

/// `failed` is replayable too — forcing an immediate retry rather than
/// waiting out the row's own backoff.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn replaying_a_failed_attempt_resets_it_to_pending() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id).await;
    let seeded = seed_attempt_in_state(
        &db,
        &endpoint.id,
        "cmsgreplayxfaild0000000",
        AttemptState::failed,
    )
    .await;
    assert_eq!(seeded.state, AttemptState::failed);

    // cratestack 0.7.13 (cratestack#512): see the identical comment on the
    // test above.
    let procedures = Procedures::new(test_pepper());
    let ctx = developer_with_webhook_manage();
    let args = replay_webhook_attempt::Args {
        args: schema::ReplayWebhookAttemptInput {
            attemptId: seeded.id.clone(),
        },
    };
    let replayed = replay_webhook_attempt::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.replay_webhook_attempt(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("replaying a failed attempt must succeed");

    assert_eq!(replayed.state, AttemptState::pending);
    assert_eq!(replayed.attempts, 0);
}

/// The circuit-breaker decision this story had to make explicit: an
/// operator replaying is treated as "I fixed the receiving end," so the
/// *endpoint's* breaker resets too, not just the one attempt — otherwise
/// `claim.rs`'s own health filter would silently keep excluding this row
/// (and every other stuck row against the same endpoint) even after the
/// replay "succeeded".
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn replay_also_clears_the_endpoints_open_circuit_breaker() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id).await;

    db.webhook_endpoint()
        .update(endpoint.id.clone())
        .set(UpdateWebhookEndpointInput {
            consecutiveFailures: Some(25),
            circuitOpenUntil: Some(Some(Utc::now() + chrono::Duration::minutes(15))),
            ..Default::default()
        })
        // #59: WebhookEndpoint is now @version'd.
        .if_match(endpoint.version)
        .run(&sys())
        .await
        .expect("tripping the endpoint's circuit breaker for the test");

    let seeded = seed_attempt_in_state(
        &db,
        &endpoint.id,
        "cmsgreplayxbreaker00000",
        AttemptState::dead,
    )
    .await;

    // cratestack 0.7.13 (cratestack#512): see the identical comment above.
    let procedures = Procedures::new(test_pepper());
    let ctx = developer_with_webhook_manage();
    let args = replay_webhook_attempt::Args {
        args: schema::ReplayWebhookAttemptInput {
            attemptId: seeded.id.clone(),
        },
    };
    replay_webhook_attempt::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.replay_webhook_attempt(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("replaying against a circuit-open endpoint must still succeed");

    let reread = db
        .webhook_endpoint()
        .find_many()
        .where_expr(FilterExpr::from(webhook_endpoint::id().eq(endpoint.id)))
        .limit(1)
        .run(&sys())
        .await
        .expect("re-reading the endpoint")
        .into_iter()
        .next()
        .expect("the endpoint still exists");

    assert_eq!(
        reread.consecutiveFailures, 0,
        "replay must reset the endpoint's failure counter"
    );
    assert!(
        reread.circuitOpenUntil.is_none(),
        "replay must clear the endpoint's open circuit"
    );
}

/// `pending`, `delivering`, and `succeeded` are all rejected as a `409
/// Conflict` — never a `500`, per this repo's own R2 discipline (`crates/
/// sms-api/src/errors.rs::map_database_error`), and never a silent no-op.
/// `succeeded` is the load-bearing case: this story deliberately does not
/// add a `succeeded -> pending` edge (§8.5's own "Implementation, #43"
/// note explains why), so replaying an already-delivered webhook must stay
/// impossible, not just undocumented.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn replaying_a_non_replayable_attempt_is_a_conflict_not_a_crash() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id).await;
    let procedures = Procedures::new(test_pepper());

    for (label, state) in [
        ("pending", AttemptState::pending),
        ("delivering", AttemptState::delivering),
        ("succeeded", AttemptState::succeeded),
    ] {
        // `aggregateId` is a plain `TEXT` column with no format constraint
        // (unlike a `Cuid` primary key) — see `webhook_attempts_dedupe`'s
        // own definition, §2.10 — so any distinguishing string works here.
        let aggregate_id = format!("replay-conflict-{label}-{}", unique_suffix());
        let seeded = seed_attempt_in_state(&db, &endpoint.id, &aggregate_id, state).await;
        assert_eq!(seeded.state, state, "precondition for {label}");

        // cratestack 0.7.13 (cratestack#512): see the identical comment on
        // the test above.
        let ctx = developer_with_webhook_manage();
        let args = replay_webhook_attempt::Args {
            args: schema::ReplayWebhookAttemptInput {
                attemptId: seeded.id.clone(),
            },
        };
        let error = replay_webhook_attempt::invoke_with_db(&db, &args, &ctx, |authorized| {
            procedures.replay_webhook_attempt(&db, &ctx, args.clone(), authorized)
        })
        .await
        .expect_err(&format!("replaying a {label} attempt must not succeed"));

        assert!(
            matches!(error, CoolError::Conflict(_)),
            "expected a 409 Conflict replaying a {label} attempt, got {error:?}"
        );
    }
}

/// A bogus attempt id is refused, not a silent no-op.
///
/// **The expected error changed from `NotFound` to `Forbidden` in the
/// cratestack 0.7.16 bump — this is real, verified production behavior,
/// not a test-only artifact.** Before cratestack 0.7.13 (cratestack#512),
/// calling `ProcedureRegistry` methods directly silently skipped
/// `@authorize(WebhookAttempt, detail, args.attemptId)` entirely, so this
/// test only ever observed the procedure body's own internal
/// `.ok_or_else(NotFound)` lookup. Now `invoke_with_db` genuinely runs
/// `authorize_with_db` first, which executes `db.webhook_attempt().
/// authorize_detail(id, ctx)` — a real `SELECT 1 FROM webhook_attempts
/// WHERE id = $1 AND <detail policy>` preflight
/// (`cratestack-sqlx-0.7.16/src/delegate/model_authorize.rs`,
/// `src/query/support/conditions.rs`) — *before* the procedure body ever
/// runs. For a nonexistent id that query structurally cannot distinguish
/// "no row" from "row exists but policy denies" (the exact ambiguity
/// `CONTRIBUTING.md`'s own R1 section already documents for
/// `CoolError::Forbidden` on update/delete — this is the same ambiguity,
/// now reachable from a procedure's own `@authorize` preflight too), so it
/// always returns `Forbidden("detail policy denied this operation")`. The
/// procedure's own `NotFound`-producing branch is unreachable for a
/// missing id as a result — it can now only ever fire in the (vanishingly
/// unlikely) TOCTOU window where a row exists at authorize time and is
/// deleted before the procedure body's own lookup runs a moment later.
/// Confirmed live, not assumed: reverting this assertion to `NotFound`
/// reproduces `expected NotFound, got Forbidden("detail policy denied
/// this operation")` on every run.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn replaying_an_unknown_attempt_id_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — which is also what makes this test's own
    // `Forbidden` expectation (see the doc comment above) the real,
    // production-accurate outcome rather than an artifact of the direct
    // call.
    let procedures = Procedures::new(test_pepper());
    let ctx = developer_with_webhook_manage();
    let args = replay_webhook_attempt::Args {
        args: schema::ReplayWebhookAttemptInput {
            attemptId: format!("nosuchattempt{}", unique_suffix()),
        },
    };
    let error = replay_webhook_attempt::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.replay_webhook_attempt(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect_err("a nonexistent attempt id must not silently succeed");

    assert!(
        matches!(error, CoolError::Forbidden(_)),
        "expected Forbidden (the @authorize detail-policy preflight denying a nonexistent row — \
         see this test's own doc comment), got {error:?}"
    );
}

/// Layer 2 (§5.1): a caller with no `webhook:manage` permission is denied
/// before the procedure touches the database at all — proven by pointing
/// it at an attempt id that doesn't even exist and confirming the error is
/// still `Forbidden`, not `NotFound` (which would mean the permission
/// check was skipped and the lookup ran anyway).
///
/// **Rewritten for the cratestack 0.7.16 bump — no longer points at a
/// nonexistent attempt id.** Same root cause as
/// `replaying_an_unknown_attempt_id_is_refused`'s own doc comment:
/// `invoke_with_db` now genuinely runs `@authorize(WebhookAttempt, detail,
/// args.attemptId)` as part of Layer 1, *before* this procedure's own
/// Layer 2 `require_permission(ctx, "webhook:manage")` ever runs. Pointing
/// this test at a nonexistent id meant Layer 1's own preflight denied it
/// first, every time, regardless of the caller's actual permissions — so
/// the test could no longer prove what its own name claims. Confirmed
/// live before fixing: with the old nonexistent-id version restored
/// temporarily, this test failed with `expected the denial to name the
/// missing permission: detail policy denied this operation`.
///
/// The fix: seed a real, `failed` (replayable) attempt. `WebhookAttempt
/// .detail`'s own `@@allow` (`schema.cstack`) is `auth().kind == "user" ||
/// endpoint.appId == auth().appId || hasRole('system')` —
/// `developer_without_permission()` is `kind: PrincipalKind::User`, which
/// already satisfies that clause unconditionally (irrespective of which
/// app the endpoint belongs to), so Layer 1 passes and Layer 2's own
/// `require_permission` is what actually produces the denial.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn replay_denies_a_caller_with_no_webhook_manage_permission() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let suffix = unique_suffix();
    let app_id = seed_app(&db, &suffix).await;
    let endpoint = seed_endpoint(&db, &suffix, &app_id).await;
    let attempt = seed_attempt_in_state(
        &db,
        &endpoint.id,
        &format!("msg-{suffix}"),
        AttemptState::failed,
    )
    .await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`, which runs the real Layer 1 `@allow`/`@authorize`
    // checks first — `kind == "user"` already admits this caller at both
    // (`schema.cstack`'s `replayWebhookAttempt` `@allow` and
    // `WebhookAttempt.detail`'s own `@@allow`, per the doc comment above),
    // so this reaches Layer 2.
    let procedures = Procedures::new(test_pepper());
    let ctx = developer_without_permission();
    let args = replay_webhook_attempt::Args {
        args: schema::ReplayWebhookAttemptInput {
            attemptId: attempt.id.clone(),
        },
    };
    let error = replay_webhook_attempt::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.replay_webhook_attempt(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect_err("a caller with no webhook:manage permission must be denied");

    assert!(
        matches!(error, CoolError::Forbidden(_)),
        "expected Forbidden, got {error:?}"
    );
    if let CoolError::Forbidden(message) = error {
        assert!(
            message.contains("webhook:manage"),
            "expected the denial to name the missing permission: {message}"
        );
    }
}
