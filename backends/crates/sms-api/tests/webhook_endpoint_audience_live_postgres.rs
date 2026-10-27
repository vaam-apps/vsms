//! A `sealed` endpoint's audience is the one thing that tells one receiver's
//! deliveries from another's (the gateway signs every app's with one key), so the database
//! hands each name out once: a second `WebhookEndpoint` cannot take an
//! audience already in use, whichever app it belongs to, on create or on
//! update. Also end to end through the delegate: the audience `vsms` is refused
//! before the database is asked, and a URL that cannot be sealed is refused by
//! the database for a `sealed` endpoint alone (an `hmac` one keeps it).
//!
//! Ignored by default, same convention as this crate's other live suites:
//!
//! ```bash
//! cargo test -p sms-api --test webhook_endpoint_audience_live_postgres -- --ignored
//! ```

use cratestack::CratestackContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, Cratestack, CreateWebhookEndpointInput, UpdateWebhookEndpointInput, webhook_endpoint,
};

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> CratestackContext {
    Principal {
        sub: "webhook-audience-test-owner".to_owned(),
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
            name: "audience test app".to_owned(),
            slug: format!("audience-test-{}", unique_suffix()),
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

fn endpoint(app_id: &str, url: &str, audience: &str) -> CreateWebhookEndpointInput {
    CreateWebhookEndpointInput {
        appId: app_id.to_owned(),
        url: url.to_owned(),
        deliveryMode: schema::WebhookDeliveryMode::sealed,
        audience: Some(audience.to_owned()),
        secret: None,
        prevSecret: None,
        secretRotatedAt: None,
        eventTypes: " message.delivered ".to_owned(),
        maskRecipient: true,
        maxAttempts: 8,
        circuitOpenUntil: None,
    }
}

/// The CHECK that holds a `sealed` endpoint's URL to a route a delivery can bind.
const CHECK: &str = "webhook_endpoints_url_sealable_for_sealed_check";

fn is_unique_violation(error: &cratestack::CratestackError) -> bool {
    error.db_sqlstate() == Some("23505")
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_second_endpoint_cannot_take_an_audience_already_in_use() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db).await;
    let other_app = seed_app(&db).await;
    let audience = format!("victim-{}", unique_suffix());

    let first = db
        .webhook_endpoint()
        .create(endpoint(
            &app,
            "https://victim.example/webhooks/vsms",
            &audience,
        ))
        .run(&owner())
        .await
        .expect("the first endpoint takes the name");

    // The attack the unique index closes: another app's endpoint, at a URL of
    // the attacker's, naming the victim's audience.
    let attacker = db
        .webhook_endpoint()
        .create(endpoint(
            &other_app,
            "https://evil.example/webhooks/vsms",
            &audience,
        ))
        .run(&owner())
        .await
        .expect_err("a second endpoint with the same audience is refused");
    assert!(is_unique_violation(&attacker), "{attacker:?}");

    // Same app, another URL: still the same name.
    let same_app = db
        .webhook_endpoint()
        .create(endpoint(
            &app,
            "https://elsewhere.example/webhooks/vsms",
            &audience,
        ))
        .run(&owner())
        .await
        .expect_err("the same app cannot hold a name twice either");
    assert!(is_unique_violation(&same_app), "{same_app:?}");

    // Nor can an existing endpoint be renamed onto it.
    let mine = db
        .webhook_endpoint()
        .create(endpoint(
            &other_app,
            "https://mine.example/webhooks/vsms",
            &format!("mine-{}", unique_suffix()),
        ))
        .run(&owner())
        .await
        .expect("an endpoint with its own name");
    let renamed = db
        .webhook_endpoint()
        .update(mine.id.clone())
        .set(UpdateWebhookEndpointInput {
            audience: Some(Some(audience.clone())),
            ..Default::default()
        })
        .if_match(mine.version)
        .run(&owner())
        .await
        .expect_err("renaming onto a taken name is refused");
    assert!(is_unique_violation(&renamed), "{renamed:?}");

    // The first endpoint still owns it, and a free name is free.
    let rows = db
        .webhook_endpoint()
        .find_many()
        .where_expr(webhook_endpoint::audience().eq(audience.clone()).into())
        .run(&owner())
        .await
        .expect("reading back");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, first.id);
    db.webhook_endpoint()
        .create(endpoint(
            &other_app,
            "https://free.example/webhooks/vsms",
            &format!("free-{}", unique_suffix()),
        ))
        .run(&owner())
        .await
        .expect("a name nobody holds");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_sealed_endpoint_refuses_an_unsealable_url_and_an_hmac_one_keeps_it() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db).await;
    for url in [
        "https://h.example/hook?x=1",
        "https://u:pw@h.example/hook",
        "https://h.example/{tenant}",
        "ftp://h.example/hook",
    ] {
        // A sealed endpoint: the database says no, naming the CHECK, and
        // `map_database_error` turns that into the 422 a procedure answers.
        let refused = db
            .webhook_endpoint()
            .create(endpoint(&app, url, &format!("url-{}", unique_suffix())))
            .run(&owner())
            .await;
        let error = refused.expect_err(&format!("{url} should be refused for sealed"));
        assert_eq!(error.db_sqlstate(), Some("23514"), "{url}: {error:?}");
        assert_eq!(error.db_constraint(), Some(CHECK), "{url}");
        assert_eq!(sms_api::map_database_error(error).status_code(), 422);

        // An hmac endpoint binds no route: it keeps whatever it was given.
        let hmac = db
            .webhook_endpoint()
            .create(CreateWebhookEndpointInput {
                deliveryMode: schema::WebhookDeliveryMode::hmac,
                audience: None,
                secret: Some("whsec_url_keeper_0123456789abcdef0123456789".to_owned()),
                ..endpoint(&app, url, "unused")
            })
            .run(&owner())
            .await;
        assert!(hmac.is_ok(), "{url} should be kept by hmac: {hmac:?}");
    }

    let vsms = db
        .webhook_endpoint()
        .create(endpoint(&app, "https://h.example/hook", "vsms"))
        .run(&owner())
        .await;
    assert!(vsms.is_err());
    assert!(!vsms.as_ref().is_err_and(is_unique_violation));

    // And an update cannot move a stored sealed endpoint onto an unsealable
    // URL, while an hmac endpoint's moves freely.
    let stored = db
        .webhook_endpoint()
        .create(endpoint(
            &app,
            "https://h.example/hook",
            &format!("ok-{}", unique_suffix()),
        ))
        .run(&owner())
        .await
        .expect("a sealable URL");
    let moved = db
        .webhook_endpoint()
        .update(stored.id.clone())
        .set(UpdateWebhookEndpointInput {
            url: Some("https://h.example/hook#frag".to_owned()),
            ..Default::default()
        })
        .if_match(stored.version)
        .run(&owner())
        .await;
    let error = moved.expect_err("a sealed endpoint cannot move onto a fragment");
    assert_eq!(error.db_constraint(), Some(CHECK), "{error:?}");

    let legacy = db
        .webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            deliveryMode: schema::WebhookDeliveryMode::hmac,
            audience: None,
            secret: Some("whsec_url_mover_0123456789abcdef0123456789".to_owned()),
            ..endpoint(&app, "https://h.example/hook", "unused")
        })
        .run(&owner())
        .await
        .expect("an hmac endpoint");
    db.webhook_endpoint()
        .update(legacy.id.clone())
        .set(UpdateWebhookEndpointInput {
            url: Some("https://h.example/hook?token=abc#frag".to_owned()),
            ..Default::default()
        })
        .if_match(legacy.version)
        .run(&owner())
        .await
        .expect("an hmac endpoint's URL moves onto a query string");
}
