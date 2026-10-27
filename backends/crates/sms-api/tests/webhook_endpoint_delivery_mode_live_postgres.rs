//! An endpoint's `deliveryMode` and the columns it decides (VS-8R), through the
//! generated delegate against a real, fully migrated Postgres.
//!
//! - **`sealed` needs an audience and holds no secret; `hmac` needs a secret.**
//!   The database says so with CHECKs (`0010_sealed_webhooks`), and a write that
//!   breaks one is refused with SQLSTATE `23514` naming the constraint. (Through
//!   the generated REST routes that surfaces as a database error; the console
//!   checks the same rules before it asks, `frontends/packages/gateway/src/
//!   webhook-rules.ts`.)
//! - **A mode switch is one update.** `hmac -> sealed` names an audience and
//!   clears all three secret columns with explicit `null`s; `sealed -> hmac`
//!   supplies a secret. Either half left out is refused and changes nothing.
//! - **A secret never reaches the audit history again.** `@sensitive` redacts
//!   `secret` and `prevSecret` in the `@@audit` snapshots of every create and
//!   update; before 0010's schema every HMAC secret this table held was written
//!   there in plaintext.
//!
//! Ignored by default, same convention as this crate's other live suites:
//!
//! ```bash
//! cargo test -p sms-api --test webhook_endpoint_delivery_mode_live_postgres -- --ignored
//! ```

use chrono::Utc;
use cratestack::CratestackContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, CreateWebhookEndpointInput, UpdateWebhookEndpointInput, WebhookDeliveryMode,
};

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> CratestackContext {
    Principal {
        sub: "webhook-delivery-mode-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
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

async fn seed_app(db: &Cratestack) -> String {
    db.app()
        .create(schema::CreateAppInput {
            name: "delivery mode test app".to_owned(),
            slug: format!("delivery-mode-{}", unique_suffix()),
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

fn sealed(app_id: &str, audience: Option<String>) -> CreateWebhookEndpointInput {
    CreateWebhookEndpointInput {
        appId: app_id.to_owned(),
        url: "https://receiver.example/webhooks/vsms".to_owned(),
        deliveryMode: WebhookDeliveryMode::sealed,
        audience,
        eventTypes: " message.delivered ".to_owned(),
        secret: None,
        prevSecret: None,
        secretRotatedAt: None,
        maskRecipient: true,
        maxAttempts: 8,
        circuitOpenUntil: None,
    }
}

fn hmac(app_id: &str, secret: Option<String>) -> CreateWebhookEndpointInput {
    CreateWebhookEndpointInput {
        deliveryMode: WebhookDeliveryMode::hmac,
        audience: None,
        secret,
        ..sealed(app_id, None)
    }
}

async fn create(
    db: &Cratestack,
    input: CreateWebhookEndpointInput,
) -> Result<schema::WebhookEndpoint, cratestack::CratestackError> {
    db.webhook_endpoint().create(input).run(&owner()).await
}

async fn update(
    db: &Cratestack,
    endpoint: &schema::WebhookEndpoint,
    input: UpdateWebhookEndpointInput,
) -> Result<schema::WebhookEndpoint, cratestack::CratestackError> {
    db.webhook_endpoint()
        .update(endpoint.id.clone())
        .set(input)
        .if_match(endpoint.version)
        .run(&owner())
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

fn refused_by(
    result: Result<schema::WebhookEndpoint, cratestack::CratestackError>,
    constraint: &str,
) {
    let error = result.expect_err("the database must refuse this row");
    assert_eq!(error.db_sqlstate(), Some("23514"), "{error:?}");
    assert_eq!(error.db_constraint(), Some(constraint), "{error:?}");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_sealed_endpoint_needs_an_audience_and_holds_no_secret() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db).await;

    refused_by(
        create(&db, sealed(&app, None)).await,
        "webhook_endpoints_audience_required_for_sealed_check",
    );
    refused_by(
        create(
            &db,
            CreateWebhookEndpointInput {
                secret: Some("whsec_smuggled".to_owned()),
                ..sealed(&app, Some(format!("recv-{}", unique_suffix())))
            },
        )
        .await,
        "webhook_endpoints_delivery_mode_sealed_holds_no_secret_check",
    );
    refused_by(
        create(
            &db,
            CreateWebhookEndpointInput {
                prevSecret: Some("whsec_smuggled_old".to_owned()),
                secretRotatedAt: Some(Utc::now()),
                ..sealed(&app, Some(format!("recv-{}", unique_suffix())))
            },
        )
        .await,
        "webhook_endpoints_delivery_mode_sealed_holds_no_secret_check",
    );

    let created = create(&db, sealed(&app, Some(format!("recv-{}", unique_suffix()))))
        .await
        .expect("a sealed endpoint with an audience and no secret");
    assert_eq!(created.deliveryMode, WebhookDeliveryMode::sealed);
    assert_eq!(created.secret, None);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_hmac_endpoint_needs_a_secret_and_no_audience() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db).await;

    refused_by(
        create(&db, hmac(&app, None)).await,
        "webhook_endpoints_delivery_mode_hmac_needs_secret_check",
    );

    // The create response carries the secret: it is how the caller learns the
    // one value the receiver has to be given.
    let secret = sms_webhook::generate_secret();
    let created = create(&db, hmac(&app, Some(secret.clone())))
        .await
        .expect("an hmac endpoint with a secret");
    assert_eq!(created.deliveryMode, WebhookDeliveryMode::hmac);
    assert_eq!(created.secret.as_deref(), Some(secret.as_str()));
    assert_eq!(created.audience, None);

    // Any number of hmac endpoints have no audience: NULLs do not collide.
    create(&db, hmac(&app, Some(sms_webhook::generate_secret())))
        .await
        .expect("a second audience-less hmac endpoint");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn switching_hmac_to_sealed_names_an_audience_and_clears_every_secret() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db).await;
    let rotated = CreateWebhookEndpointInput {
        prevSecret: Some("whsec_previous".to_owned()),
        secretRotatedAt: Some(Utc::now()),
        ..hmac(&app, Some("whsec_current".to_owned()))
    };
    let endpoint = create(&db, rotated).await.expect("a rotated hmac endpoint");
    let audience = format!("recv-{}", unique_suffix());

    // The mode alone, an audience without clearing, clearing without an
    // audience: each half is refused, and nothing is written.
    let mode_only = update(
        &db,
        &endpoint,
        UpdateWebhookEndpointInput {
            deliveryMode: Some(WebhookDeliveryMode::sealed),
            ..Default::default()
        },
    )
    .await;
    refused_by(
        mode_only,
        "webhook_endpoints_audience_required_for_sealed_check",
    );
    let no_clearing = update(
        &db,
        &endpoint,
        UpdateWebhookEndpointInput {
            deliveryMode: Some(WebhookDeliveryMode::sealed),
            audience: Some(Some(audience.clone())),
            ..Default::default()
        },
    )
    .await;
    refused_by(
        no_clearing,
        "webhook_endpoints_delivery_mode_sealed_holds_no_secret_check",
    );
    let no_audience = update(
        &db,
        &endpoint,
        UpdateWebhookEndpointInput {
            deliveryMode: Some(WebhookDeliveryMode::sealed),
            secret: Some(None),
            prevSecret: Some(None),
            secretRotatedAt: Some(None),
            ..Default::default()
        },
    )
    .await;
    refused_by(
        no_audience,
        "webhook_endpoints_audience_required_for_sealed_check",
    );
    let untouched = reread(&db, &endpoint.id).await;
    assert_eq!(untouched.deliveryMode, WebhookDeliveryMode::hmac);
    assert_eq!(untouched.secret.as_deref(), Some("whsec_current"));
    assert_eq!(untouched.prevSecret.as_deref(), Some("whsec_previous"));
    assert_eq!(untouched.version, endpoint.version, "nothing was written");

    let switched = update(
        &db,
        &endpoint,
        UpdateWebhookEndpointInput {
            deliveryMode: Some(WebhookDeliveryMode::sealed),
            audience: Some(Some(audience.clone())),
            secret: Some(None),
            prevSecret: Some(None),
            secretRotatedAt: Some(None),
            ..Default::default()
        },
    )
    .await
    .expect("one update that names the audience and clears the secrets");
    assert_eq!(switched.deliveryMode, WebhookDeliveryMode::sealed);
    assert_eq!(switched.audience.as_deref(), Some(audience.as_str()));
    assert_eq!(switched.secret, None);
    assert_eq!(switched.prevSecret, None);
    assert_eq!(switched.secretRotatedAt, None);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn switching_sealed_to_hmac_supplies_a_secret() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db).await;
    let audience = format!("recv-{}", unique_suffix());
    let endpoint = create(&db, sealed(&app, Some(audience.clone())))
        .await
        .expect("a sealed endpoint");

    let without_secret = update(
        &db,
        &endpoint,
        UpdateWebhookEndpointInput {
            deliveryMode: Some(WebhookDeliveryMode::hmac),
            ..Default::default()
        },
    )
    .await;
    refused_by(
        without_secret,
        "webhook_endpoints_delivery_mode_hmac_needs_secret_check",
    );
    assert_eq!(
        reread(&db, &endpoint.id).await.deliveryMode,
        WebhookDeliveryMode::sealed
    );

    let secret = sms_webhook::generate_secret();
    let switched = update(
        &db,
        &endpoint,
        UpdateWebhookEndpointInput {
            deliveryMode: Some(WebhookDeliveryMode::hmac),
            secret: Some(Some(secret.clone())),
            ..Default::default()
        },
    )
    .await
    .expect("one update that supplies the secret");
    assert_eq!(switched.deliveryMode, WebhookDeliveryMode::hmac);
    assert_eq!(switched.secret.as_deref(), Some(secret.as_str()));
    // The audience was not asked to go, and keeps its name (a switch back
    // would want it); an hmac endpoint simply does not use it.
    assert_eq!(switched.audience.as_deref(), Some(audience.as_str()));
}

/// `psql` against the harness database, for the one table the typed client does
/// not model: the framework's own `cratestack_audit`.
async fn audit_after_snapshots(endpoint_id: &str) -> Vec<String> {
    let database_url = sms_test_support::database_url().await;
    let output = std::process::Command::new("psql")
        .arg(database_url)
        .args(["-v", "ON_ERROR_STOP=1", "-q", "-At", "-c"])
        .arg(format!(
            "SELECT \"after\"::text FROM cratestack_audit \
             WHERE model = 'WebhookEndpoint' AND primary_key::text LIKE '%{endpoint_id}%' \
             ORDER BY occurred_at"
        ))
        .output()
        .expect("spawning `psql`");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_secret_is_redacted_from_every_audit_snapshot() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db).await;
    let secret = format!("whsec_audit_{}", unique_suffix());
    let endpoint = create(&db, hmac(&app, Some(secret.clone())))
        .await
        .expect("an hmac endpoint");
    let rotated_to = format!("whsec_audit_rotated_{}", unique_suffix());
    update(
        &db,
        &endpoint,
        UpdateWebhookEndpointInput {
            secret: Some(Some(rotated_to.clone())),
            prevSecret: Some(Some(secret.clone())),
            secretRotatedAt: Some(Some(Utc::now())),
            ..Default::default()
        },
    )
    .await
    .expect("a rotation");

    let snapshots = audit_after_snapshots(&endpoint.id).await;
    assert!(
        snapshots.len() >= 2,
        "a create and an update were audited: {snapshots:?}"
    );
    for snapshot in &snapshots {
        assert!(
            !snapshot.contains(&secret) && !snapshot.contains(&rotated_to),
            "a secret reached the audit history: {snapshot}"
        );
        assert!(snapshot.contains("[redacted-sensitive]"), "{snapshot}");
    }
}
