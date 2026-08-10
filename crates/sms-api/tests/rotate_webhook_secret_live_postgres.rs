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
//! ```bash
//! cargo test -p sms-api --test rotate_webhook_secret_live_postgres -- --ignored
//! ```

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::CoolContext;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, procedures::rotate_webhook_secret, procedures::ProcedureRegistry, Cratestack,
};
use sms_api::{HashPepper, Procedures};

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests — run concurrently by Rust's default multi-threaded test
/// harness — can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape at the
/// same instant. See `crates/sms-worker/tests/claim_live_postgres.rs`'s
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
    let rotated = Procedures::new(test_pepper())
        .rotate_webhook_secret(
            &db,
            &owner(),
            rotate_webhook_secret::Args {
                args: schema::EndpointInput {
                    endpointId: endpoint.id.clone(),
                },
            },
        )
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

    let first = procedures
        .rotate_webhook_secret(&db, &owner(), args())
        .await
        .expect("first rotation");
    assert_eq!(first.prevSecret.as_deref(), Some("generation-zero-secret"));

    let second = procedures
        .rotate_webhook_secret(&db, &owner(), args())
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

    let error = Procedures::new(test_pepper())
        .rotate_webhook_secret(
            &db,
            &owner(),
            rotate_webhook_secret::Args {
                args: schema::EndpointInput {
                    endpointId: format!("nosuchendpoint{}", unique_suffix()),
                },
            },
        )
        .await
        .expect_err("a nonexistent endpoint id must not silently succeed");

    assert!(
        matches!(error, cratestack::CoolError::NotFound(_)),
        "expected NotFound, got {error:?}"
    );
}
