//! `rotateWebhookSecret` (#41) against a real, fully migrated Postgres —
//! the actual `ProcedureRegistry` trait method, not `Procedures::rotate_secret`
//! called directly, the same discipline `send_message_live_postgres.rs`
//! documents in its own module doc.
//!
//! This is also the live proof behind `schema.cstack`'s own new
//! `hasRole('system')` clause on `WebhookEndpoint`'s `read`/`update`
//! policy: before that clause existed, every assertion below would have
//! failed not with a clear error but with a silent empty read/a denied
//! write — the exact failure mode `AGENTS.md`'s "Invariants that fail the
//! build rather than production" section and
//! `system_context_golden_list_live_postgres.rs` both describe.
//!
//! #193 added a Layer 2 `require_permission(ctx, "webhook:manage")` gate
//! to `rotate_secret`, matching `replayWebhookAttempt`'s (#43, #191) —
//! `rotate_denies_a_caller_with_no_webhook_manage_permission` below is its
//! denial-path proof, the same shape
//! `replay_webhook_attempt_live_postgres.rs`'s own denial test already
//! established.
//!
//! ```bash
//! cargo test -p sms-api --test rotate_webhook_secret_live_postgres -- --ignored
//! ```

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, procedures::rotate_webhook_secret, procedures::ProcedureRegistry, Cratestack,
};
use sms_api::{HashPepper, Procedures};

/// A human caller of the one role that could plausibly rotate a secret
/// (`developer` — §5.2), but carrying no `perms` claim at all — the exact
/// "an omitted scope yields denial" shape §5.2 documents, extended to
/// `perms` by `require_permission`'s own doc comment. Mirrors
/// `replay_webhook_attempt_live_postgres.rs`'s own
/// `developer_without_permission`, since #193 gave `rotateWebhookSecret`
/// the same Layer 2 gate `replayWebhookAttempt` already had.
fn developer_without_permission() -> CoolContext {
    Principal {
        sub: "rotate-webhook-secret-test-developer-no-perms".to_owned(),
        kind: PrincipalKind::User,
        role: "developer".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

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
        sub: "rotate-webhook-secret-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// #193: `owner()` above has no `perms` claim — fine for seeding through
/// generated CRUD (Layer 1 only), but `rotate_secret` now calls
/// `require_permission(ctx, "webhook:manage")` first, and a
/// test-constructed context (unlike a real issued token — no human-login
/// flow exists yet, #97/#98) carries no claim `into_context()` doesn't put
/// there. `owner`/`admin` hold `webhook:manage` implicitly as part of
/// "everything"/"all" per §5.2's own table; this is that same role, with
/// the claim spelled out by hand for a direct procedure call, the same way
/// `replay_webhook_attempt_live_postgres.rs`'s own
/// `developer_with_webhook_manage` does for its sibling procedure.
fn owner_with_webhook_manage() -> CoolContext {
    let mut ctx = owner();
    ctx.extensions.insert(
        "perms".to_owned(),
        Value::List(vec![Value::String("webhook:manage".to_owned())]),
    );
    ctx
}

fn test_pepper() -> HashPepper {
    HashPepper::new("rotate-webhook-secret-live-postgres-test-pepper-well-over-minimum")
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

async fn seed_app(db: &Cratestack) -> String {
    let suffix = unique_suffix();
    db.app()
        .create(schema::CreateAppInput {
            name: "rotate-webhook-secret live test app".to_owned(),
            slug: format!("rotate-webhook-secret-test-{}", suffix.to_lowercase()),
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

/// A fresh `WebhookEndpoint` with `secret` and no `prevSecret` yet — the
/// state every real endpoint starts in before its first rotation.
async fn seed_endpoint(db: &Cratestack, app_id: &str, secret: &str) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(schema::CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: "https://example.test/webhooks/vsms".to_owned(),
            eventTypes: sms_core::pack(["message.delivered"]).expect("a static literal packs"),
            secret: secret.to_owned(),
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

/// The headline case: rotating a fresh endpoint moves its current secret
/// to `prevSecret`, mints a fresh one in `secret`, and stamps
/// `secretRotatedAt`.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn rotating_moves_the_current_secret_to_prev_and_mints_a_fresh_one() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let endpoint = seed_endpoint(&db, &app_id, "original-secret-before-any-rotation").await;

    let before = Utc::now();
    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — the "sanctioned way to invoke a procedure from
    // non-HTTP code" per that function's own doc comment.
    let procedures = Procedures::new(test_pepper());
    let ctx = owner_with_webhook_manage();
    let args = rotate_webhook_secret::Args {
        args: schema::EndpointInput {
            endpointId: endpoint.id.clone(),
        },
    };
    let rotated = rotate_webhook_secret::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.rotate_webhook_secret(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("rotating a fresh endpoint's secret");

    assert_eq!(rotated.id, endpoint.id);
    assert_ne!(
        rotated.secret, "original-secret-before-any-rotation",
        "the secret must actually change"
    );
    assert!(
        rotated.secret.starts_with("whsec_"),
        "generate_secret's documented shape — got {:?}",
        rotated.secret
    );
    assert_eq!(
        rotated.prevSecret.as_deref(),
        Some("original-secret-before-any-rotation"),
        "the pre-rotation secret must survive in prevSecret for the overlap window"
    );
    let rotated_at = rotated
        .secretRotatedAt
        .expect("secretRotatedAt must be stamped by a rotation");
    assert!(
        rotated_at >= before,
        "secretRotatedAt should be stamped at rotation time, not left stale"
    );
}

/// Rotating twice in a row must shift the overlap window forward, not
/// just repeat the same prev/current pair — `prevSecret` after the second
/// rotation is the secret the *first* rotation minted, not the original.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn rotating_twice_shifts_the_overlap_window_forward() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let endpoint = seed_endpoint(&db, &app_id, "generation-zero-secret").await;
    let procedures = Procedures::new(test_pepper());

    let args = || rotate_webhook_secret::Args {
        args: schema::EndpointInput {
            endpointId: endpoint.id.clone(),
        },
    };
    let ctx = owner_with_webhook_manage();

    // cratestack 0.7.13 (cratestack#512): see the identical comment on the
    // test above.
    let first_args = args();
    let first = rotate_webhook_secret::invoke_with_db(&db, &first_args, &ctx, |authorized| {
        procedures.rotate_webhook_secret(&db, &ctx, first_args.clone(), authorized)
    })
    .await
    .expect("first rotation");
    assert_eq!(first.prevSecret.as_deref(), Some("generation-zero-secret"));

    let second_args = args();
    let second = rotate_webhook_secret::invoke_with_db(&db, &second_args, &ctx, |authorized| {
        procedures.rotate_webhook_secret(&db, &ctx, second_args.clone(), authorized)
    })
    .await
    .expect("second rotation");
    assert_eq!(
        second.prevSecret.as_deref(),
        Some(first.secret.as_str()),
        "the second rotation's prevSecret must be the FIRST rotation's fresh secret, \
         not the original generation-zero one — the overlap window shifts, it doesn't reset"
    );
    assert_ne!(
        second.secret, first.secret,
        "each rotation must mint a genuinely new secret"
    );
    assert_ne!(
        second.secret, "generation-zero-secret",
        "the original secret must not resurface"
    );
}

/// A bogus endpoint id is a clear `NotFound`, not a silent no-op or a
/// generic database error — the exact distinction the missing
/// `hasRole('system')` clause this PR adds would otherwise blur (a
/// missing-policy empty read looks identical to a missing-row empty read
/// from the caller's side, which is precisely why this test exists
/// alongside the golden-list guard rather than instead of it).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn rotating_an_unknown_endpoint_id_is_not_found() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // cratestack 0.7.13 (cratestack#512): see the identical comment on the
    // test above.
    let procedures = Procedures::new(test_pepper());
    let ctx = owner_with_webhook_manage();
    let args = rotate_webhook_secret::Args {
        args: schema::EndpointInput {
            endpointId: format!("nosuchendpoint{}", unique_suffix()),
        },
    };
    let error = rotate_webhook_secret::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.rotate_webhook_secret(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect_err("a nonexistent endpoint id must not silently succeed");

    assert!(
        matches!(error, cratestack::CoolError::NotFound(_)),
        "expected NotFound, got {error:?}"
    );
}

/// #193: Layer 2 (§5.1) — a caller with no `webhook:manage` permission is
/// denied before the procedure touches the database at all. Proven the
/// same way `replay_webhook_attempt_live_postgres.rs`'s own
/// `replay_denies_a_caller_with_no_webhook_manage_permission` proves it for
/// the sibling procedure: point the call at an endpoint id that doesn't
/// even exist, and confirm the error is `Forbidden`, not `NotFound` — a
/// `NotFound` here would mean the permission check was skipped and the
/// lookup ran anyway.
///
/// This is the denial half AGENTS.md and the issue both call for. The
/// *allow* half has no live token to prove it with in this deployment —
/// `GatewayAuth` never mints a human-role token today (#97/#98's scope
/// cut) — so, same as `replayWebhookAttempt`'s own coverage, only denial
/// is exercised live here.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn rotate_denies_a_caller_with_no_webhook_manage_permission() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`, which runs the real Layer 1 `@allow` check first —
    // `hasRole('developer')` already admits this caller there
    // (`schema.cstack`'s `rotateWebhookSecret` `@allow`), so this stays a
    // genuine Layer 2 (`require_permission`) denial, not a Layer 1 one.
    let procedures = Procedures::new(test_pepper());
    let ctx = developer_without_permission();
    let args = rotate_webhook_secret::Args {
        args: schema::EndpointInput {
            endpointId: format!("irrelevant-the-gate-must-fire-first{}", unique_suffix()),
        },
    };
    let error = rotate_webhook_secret::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.rotate_webhook_secret(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect_err("a caller with no webhook:manage permission must be denied");

    assert!(
        matches!(error, cratestack::CoolError::Forbidden(_)),
        "expected Forbidden, got {error:?}"
    );
    if let cratestack::CoolError::Forbidden(message) = error {
        assert!(
            message.contains("webhook:manage"),
            "expected the denial to name the missing permission: {message}"
        );
    }
}
