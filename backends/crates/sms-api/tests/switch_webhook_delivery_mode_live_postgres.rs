//! `switchWebhookDeliveryMode` (VS-8R) against a real, fully migrated Postgres,
//! through the `ProcedureRegistry` trait method the way
//! `rotate_webhook_secret_live_postgres.rs` drives its sibling.
//!
//! A raw `PATCH` of an endpoint's mode has to move four columns in step, and a
//! row that disagrees with its mode is refused by a CHECK that the generated
//! routes answer with a database error (500). This procedure is the answer that
//! validates first (422), does the bookkeeping, and mints the secret:
//!
//! - **to `hmac`**: a `whsec_` secret is minted server-side, the audience is
//!   freed, and `prevSecret` and `secretRotatedAt` are cleared;
//! - **to `sealed`**: an audience is required, the URL must be one a delivery
//!   can be sealed for, and every secret column is cleared;
//! - **refusals** are `Validation` (422) with the row exactly as it was, a taken
//!   audience is `Conflict` (409), an unknown id `NotFound`, and a caller
//!   without `webhook:manage` `Forbidden`.
//!
//! ```bash
//! cargo test -p sms-api --test switch_webhook_delivery_mode_live_postgres -- --ignored
//! ```

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, CratestackError, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, WebhookDeliveryMode, procedures::ProcedureRegistry,
    procedures::switch_webhook_delivery_mode,
};
use sms_api::{HashPepper, Procedures};

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const ORIGINAL_SECRET: &str = "original-secret-before-any-switch-0123456789";
const ORIGINAL_PREVIOUS: &str = "previous-secret-before-any-switch-0123456789";

fn owner() -> CratestackContext {
    Principal {
        sub: "switch-delivery-mode-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// `owner` with the Layer 2 permission spelled out, as a direct procedure call
/// needs it (`rotate_webhook_secret_live_postgres.rs` has the reasoning).
fn owner_with_webhook_manage() -> CratestackContext {
    let mut ctx = owner();
    ctx.extensions.insert(
        "perms".to_owned(),
        Value::List(vec![Value::String("webhook:manage".to_owned())]),
    );
    ctx
}

fn developer_without_permission() -> CratestackContext {
    Principal {
        sub: "switch-delivery-mode-test-developer-no-perms".to_owned(),
        kind: PrincipalKind::User,
        role: "developer".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn procedures() -> Procedures {
    Procedures::new(
        HashPepper::new("switch-delivery-mode-live-postgres-test-pepper-well-over-minimum")
            .expect("test pepper meets HashPepper::new's minimum length"),
    )
}

fn unique_suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
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
    db.app()
        .create(schema::CreateAppInput {
            name: "switch-delivery-mode live test app".to_owned(),
            slug: format!("switch-mode-test-{}", unique_suffix()),
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

async fn seed_sealed(db: &Cratestack, app_id: &str, audience: &str) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(schema::CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: "https://example.test/webhooks/vsms".to_owned(),
            eventTypes: sms_core::pack(["message.delivered"]).expect("a static literal packs"),
            deliveryMode: WebhookDeliveryMode::sealed,
            audience: Some(audience.to_owned()),
            secret: None,
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding a sealed endpoint")
}

/// An `hmac` endpoint mid-rotation (a previous secret and a rotation stamp), at
/// `url`: an `hmac` endpoint may hold any of them.
async fn seed_hmac(db: &Cratestack, app_id: &str, url: &str) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .create(schema::CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: url.to_owned(),
            eventTypes: sms_core::pack(["message.delivered"]).expect("a static literal packs"),
            deliveryMode: WebhookDeliveryMode::hmac,
            audience: None,
            secret: Some(ORIGINAL_SECRET.to_owned()),
            prevSecret: Some(ORIGINAL_PREVIOUS.to_owned()),
            secretRotatedAt: Some(chrono::Utc::now()),
            maskRecipient: true,
            maxAttempts: 8,
            circuitOpenUntil: None,
        })
        .run(&owner())
        .await
        .expect("seeding an hmac endpoint")
}

async fn switch(
    db: &Cratestack,
    ctx: &CratestackContext,
    endpoint_id: &str,
    mode: WebhookDeliveryMode,
    audience: Option<&str>,
) -> Result<schema::WebhookEndpoint, CratestackError> {
    let procedures = procedures();
    let args = switch_webhook_delivery_mode::Args {
        args: schema::SwitchWebhookDeliveryModeInput {
            endpointId: endpoint_id.to_owned(),
            mode,
            audience: audience.map(str::to_owned),
        },
    };
    switch_webhook_delivery_mode::invoke_with_db(db, &args, ctx, |db, authorized| {
        let (p, c, a) = (procedures.clone(), ctx.clone(), args.clone());
        async move { p.switch_webhook_delivery_mode(&db, &c, a, authorized).await }
    })
    .await
}

async fn reread(db: &Cratestack, id: &str) -> schema::WebhookEndpoint {
    db.webhook_endpoint()
        .find_unique(id.to_owned())
        .run(&owner())
        .await
        .expect("re-reading the endpoint")
        .expect("the endpoint exists")
}

/// A refusal is a `Validation` (422) and the row is as it was, version included.
async fn assert_refused_and_unchanged(
    db: &Cratestack,
    before: &schema::WebhookEndpoint,
    mode: WebhookDeliveryMode,
    audience: Option<&str>,
    words: &str,
) {
    let error = switch(db, &owner_with_webhook_manage(), &before.id, mode, audience)
        .await
        .expect_err("this switch must be refused");
    match &error {
        CratestackError::Validation(message) => {
            assert!(message.contains(words), "{words:?} not in {message:?}");
        }
        other => panic!("expected a Validation (422), got {other:?}"),
    }
    assert_eq!(error.status_code(), 422);
    let after = reread(db, &before.id).await;
    assert_eq!(after.version, before.version, "a refusal writes nothing");
    assert_eq!(after.deliveryMode, before.deliveryMode);
    assert_eq!(after.audience, before.audience);
    assert_eq!(after.secret, before.secret);
    assert_eq!(after.prevSecret, before.prevSecret);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn sealed_to_hmac_mints_a_secret_and_frees_the_audience() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let audience = format!("switch-to-hmac-{}", unique_suffix());
    let sealed = seed_sealed(&db, &app_id, &audience).await;

    let switched = switch(
        &db,
        &owner_with_webhook_manage(),
        &sealed.id,
        WebhookDeliveryMode::hmac,
        None,
    )
    .await
    .expect("switching to hmac");

    assert_eq!(switched.deliveryMode, WebhookDeliveryMode::hmac);
    let secret = switched.secret.as_deref().expect("a minted secret");
    assert!(
        secret.starts_with("whsec_") && secret.len() == 70,
        "a minted secret is whsec_ plus 64 hex (got {} characters)",
        secret.len()
    );
    assert_eq!(switched.audience, None);
    assert_eq!(switched.prevSecret, None);
    assert_eq!(switched.secretRotatedAt, None);
    assert!(switched.version > sealed.version);
    assert_eq!(reread(&db, &sealed.id).await.secret, switched.secret);

    // The audience is free for another endpoint to take.
    let other = seed_sealed(&db, &app_id, &audience).await;
    assert_eq!(other.audience.as_deref(), Some(audience.as_str()));
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn hmac_to_sealed_names_the_audience_and_clears_every_secret_column() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let hmac = seed_hmac(&db, &app_id, "https://example.test/webhooks/vsms").await;
    assert!(hmac.prevSecret.is_some() && hmac.secretRotatedAt.is_some());
    let audience = format!("switch-to-sealed-{}", unique_suffix());

    let switched = switch(
        &db,
        &owner_with_webhook_manage(),
        &hmac.id,
        WebhookDeliveryMode::sealed,
        Some(&audience),
    )
    .await
    .expect("switching to sealed");

    assert_eq!(switched.deliveryMode, WebhookDeliveryMode::sealed);
    assert_eq!(switched.audience.as_deref(), Some(audience.as_str()));
    assert_eq!(switched.secret, None);
    assert_eq!(switched.prevSecret, None);
    assert_eq!(switched.secretRotatedAt, None);
    assert_eq!(switched.url, hmac.url);
    let stored = reread(&db, &hmac.id).await;
    assert_eq!(stored.deliveryMode, WebhookDeliveryMode::sealed);
    assert_eq!(stored.secret, None);

    // And back: a new secret, not the one it had.
    let back = switch(
        &db,
        &owner_with_webhook_manage(),
        &hmac.id,
        WebhookDeliveryMode::hmac,
        None,
    )
    .await
    .expect("switching back to hmac");
    assert_ne!(back.secret.as_deref(), Some(ORIGINAL_SECRET));
    assert_eq!(back.audience, None);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_switch_that_would_break_a_rule_is_a_422_and_changes_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let hmac = seed_hmac(&db, &app_id, "https://example.test/webhooks/vsms").await;
    let sealed = seed_sealed(
        &db,
        &app_id,
        &format!("switch-refusals-{}", unique_suffix()),
    )
    .await;

    // To sealed: an audience is required, and the name the gateway answers to
    // is not one.
    assert_refused_and_unchanged(
        &db,
        &hmac,
        WebhookDeliveryMode::sealed,
        None,
        "needs an audience",
    )
    .await;
    assert_refused_and_unchanged(
        &db,
        &hmac,
        WebhookDeliveryMode::sealed,
        Some(""),
        "needs an audience",
    )
    .await;
    assert_refused_and_unchanged(
        &db,
        &hmac,
        WebhookDeliveryMode::sealed,
        Some("vsms"),
        "audience",
    )
    .await;
    // To hmac: it has no audience to take.
    assert_refused_and_unchanged(
        &db,
        &sealed,
        WebhookDeliveryMode::hmac,
        Some("squatted"),
        "no audience",
    )
    .await;
    // To the mode it is in: nothing to switch.
    assert_refused_and_unchanged(&db, &hmac, WebhookDeliveryMode::hmac, None, "already hmac").await;
    assert_refused_and_unchanged(
        &db,
        &sealed,
        WebhookDeliveryMode::sealed,
        Some("other"),
        "already sealed",
    )
    .await;
}

/// An `hmac` endpoint may keep a URL a sealed delivery could not use (a query
/// string, credentials), and switching it to sealed says so rather than
/// tripping the CHECK. The URL is not echoed: it may carry credentials.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_legacy_hmac_url_cannot_be_switched_to_sealed_and_is_not_echoed() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let legacy = seed_hmac(
        &db,
        &app_id,
        "https://user:hunter2@example.test/webhooks/vsms?token=hunter2",
    )
    .await;

    let error = switch(
        &db,
        &owner_with_webhook_manage(),
        &legacy.id,
        WebhookDeliveryMode::sealed,
        Some(&format!("legacy-{}", unique_suffix())),
    )
    .await
    .expect_err("the URL cannot carry a sealed delivery");
    let CratestackError::Validation(message) = error else {
        panic!("expected a Validation (422), got {error:?}");
    };
    assert!(
        message.contains("cannot carry a sealed delivery"),
        "{message}"
    );
    assert!(!message.contains("hunter2"), "the URL leaked: {message}");
    assert_eq!(reread(&db, &legacy.id).await.version, legacy.version);
}

/// The audience is unique: a taken one is a 409 from the unique index, not a
/// database error.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_taken_audience_is_a_conflict() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app_id = seed_app(&db).await;
    let taken = format!("switch-taken-{}", unique_suffix());
    seed_sealed(&db, &app_id, &taken).await;
    let hmac = seed_hmac(&db, &app_id, "https://example.test/webhooks/vsms").await;

    let error = switch(
        &db,
        &owner_with_webhook_manage(),
        &hmac.id,
        WebhookDeliveryMode::sealed,
        Some(&taken),
    )
    .await
    .expect_err("the audience is taken");
    assert!(matches!(error, CratestackError::Conflict(_)), "{error:?}");
    assert_eq!(error.status_code(), 409);
    let after = reread(&db, &hmac.id).await;
    assert_eq!(after.deliveryMode, WebhookDeliveryMode::hmac);
    assert_eq!(after.secret.as_deref(), Some(ORIGINAL_SECRET));
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn an_unknown_endpoint_is_not_found() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let error = switch(
        &db,
        &owner_with_webhook_manage(),
        &format!("nosuchendpoint{}", unique_suffix()),
        WebhookDeliveryMode::hmac,
        None,
    )
    .await
    .expect_err("no such endpoint");
    assert!(matches!(error, CratestackError::NotFound(_)), "{error:?}");
}

/// Layer 2: a caller without `webhook:manage` is denied before the database is
/// asked (an unknown id would be `NotFound` otherwise).
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_caller_without_webhook_manage_is_denied_first() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let error = switch(
        &db,
        &developer_without_permission(),
        &format!("irrelevant-the-gate-must-fire-first{}", unique_suffix()),
        WebhookDeliveryMode::hmac,
        None,
    )
    .await
    .expect_err("denied");
    match error {
        CratestackError::Forbidden(message) => assert!(message.contains("webhook:manage")),
        other => panic!("expected Forbidden, got {other:?}"),
    }
}
