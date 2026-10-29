//! `auth.methods` without `hmac_webhooks` (VSA-1), over real HTTP.
//!
//! A real router (`sms_api::router`, built with `HmacWebhooks::Disabled`), a real
//! paired person holding `webhook:manage`, sealed requests, a real Postgres: the
//! four calls that would make or re-key an `hmac` endpoint, through the generated
//! `POST` and `PATCH` (the middleware, `sms_api::hmac_gate`) and through the two
//! procedures, all answer 422 with `auth_method_disabled` and write nothing; and
//! the calls an operator who switched the method off still needs keep working.
//! With the method permitted every one of them is what it was before the key
//! existed.
//!
//! ```bash
//! cargo test -p sms-api --test hmac_webhooks_gate_live_postgres -- --ignored
//! ```

#[path = "support/sealed_fixtures.rs"]
mod sealed_fixtures;

use cratestack::sqlx::postgres::PgPoolOptions;
use sealed_fixtures::{
    api_router_with, client, owner, register_app_key, seed_user_with_console_key, serve,
};
use sms_api::HmacWebhooks;
use sms_api::auth::system_context;
use sms_api::schema::{self, Cratestack, WebhookDeliveryMode};
use sms_test_support::sealed::{Reply, SealedClient, TestKey};

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// 32 characters, the least a key written through the API may have.
fn key_of_32() -> String {
    "k".repeat(32)
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

struct Deployment {
    db: Cratestack,
    base: String,
    app_id: String,
}

async fn deployment(hmac: HmacWebhooks) -> Deployment {
    let db = db().await;
    let base = serve(api_router_with(
        &db,
        "hmac-gate-live-test-key-material-well-over-the-minimum-length",
        hmac,
    ))
    .await;
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "hmac gate test app".to_owned(),
            slug: format!("hmac-gate-test-{}", unique_suffix()),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the app");
    Deployment {
        db,
        base,
        app_id: app.id,
    }
}

impl Deployment {
    /// A developer: holds `webhook:manage` and may create, update and read an
    /// endpoint.
    async fn developer(&self) -> SealedClient {
        let key = TestKey::console();
        seed_user_with_console_key(&self.db, "developer", &key).await;
        client(&self.base, &key)
    }

    /// An endpoint written around the API, as one left behind by a deployment
    /// that used to permit the method would be.
    async fn hmac_endpoint(&self) -> schema::WebhookEndpoint {
        self.db
            .webhook_endpoint()
            .create(schema::CreateWebhookEndpointInput {
                appId: self.app_id.clone(),
                url: "https://example.test/hooks".to_owned(),
                eventTypes: sms_core::pack(["message.delivered"]).expect("a static literal packs"),
                deliveryMode: WebhookDeliveryMode::hmac,
                audience: None,
                secret: Some(key_of_32()),
                prevSecret: None,
                secretRotatedAt: None,
                maskRecipient: true,
                maxAttempts: 8,
                circuitOpenUntil: None,
            })
            .run(&owner())
            .await
            .expect("seeding an hmac endpoint")
    }

    async fn sealed_endpoint(&self) -> schema::WebhookEndpoint {
        self.db
            .webhook_endpoint()
            .create(schema::CreateWebhookEndpointInput {
                appId: self.app_id.clone(),
                url: "https://example.test/sealed".to_owned(),
                eventTypes: sms_core::pack(["message.delivered"]).expect("a static literal packs"),
                deliveryMode: WebhookDeliveryMode::sealed,
                audience: Some(format!("gate-test-{}", unique_suffix())),
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

    async fn endpoint_count(&self) -> usize {
        self.db
            .webhook_endpoint()
            .find_many()
            .where_expr(cratestack::FilterExpr::from(
                schema::webhook_endpoint::appId().eq(self.app_id.clone()),
            ))
            .run(&system_context("hmac-gate-test"))
            .await
            .expect("counting the app's endpoints")
            .len()
    }

    async fn reread(&self, id: &str) -> schema::WebhookEndpoint {
        self.db
            .webhook_endpoint()
            .find_unique(id.to_owned())
            .run(&owner())
            .await
            .expect("re-reading the endpoint")
            .expect("the endpoint exists")
    }
}

fn create_body(app_id: &str, mode: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "appId": app_id,
        "url": "https://example.test/new",
        "eventTypes": " message.delivered ",
        "deliveryMode": mode,
        "maskRecipient": true,
        "maxAttempts": 8,
    });
    if mode == "hmac" {
        body["secret"] = key_of_32().into();
    } else {
        body["audience"] = format!("created-{}", unique_suffix()).into();
    }
    body
}

fn message_of(reply: &Reply) -> String {
    reply.json()["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// A 422 that leads with the stable word.
fn assert_disabled(reply: &Reply, what: &str) {
    assert_eq!(reply.status, 422, "{what}: {}", reply.text());
    assert!(
        message_of(reply).starts_with("auth_method_disabled: "),
        "{what}: {}",
        reply.text()
    );
}

async fn etag_of(api: &SealedClient, id: &str) -> String {
    let reply = api
        .request("GET", "/webhook_endpoints/{id}")
        .params(&[id])
        .send()
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    reply
        .headers
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .expect("a versioned model answers with an ETag")
        .to_owned()
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_hmac_off_create_and_update_through_the_generated_routes_are_422() {
    let _guard = TEST_MUTEX.lock().await;
    let deployment = deployment(HmacWebhooks::Disabled).await;
    let developer = deployment.developer().await;
    let left_behind = deployment.hmac_endpoint().await;
    let before = deployment.endpoint_count().await;

    let created = developer
        .request("POST", "/webhook_endpoints")
        .json(&create_body(&deployment.app_id, "hmac"))
        .send()
        .await;
    assert_disabled(&created, "POST /webhook_endpoints with deliveryMode hmac");
    assert_eq!(
        deployment.endpoint_count().await,
        before,
        "the refused create wrote no row"
    );

    let etag = etag_of(&developer, &left_behind.id).await;
    for (what, body) in [
        (
            "PATCH naming deliveryMode hmac",
            serde_json::json!({"deliveryMode": "hmac"}),
        ),
        (
            "PATCH setting a secret",
            serde_json::json!({"secret": key_of_32()}),
        ),
        (
            "PATCH setting a previous secret",
            serde_json::json!({"prevSecret": key_of_32()}),
        ),
    ] {
        let reply = developer
            .request("PATCH", "/webhook_endpoints/{id}")
            .params(&[&left_behind.id])
            .if_match(&etag)
            .json(&body)
            .send()
            .await;
        assert_disabled(&reply, what);
    }
    let after = deployment.reread(&left_behind.id).await;
    assert_eq!(
        after.version, left_behind.version,
        "a refusal writes nothing"
    );
    assert_eq!(after.secret, left_behind.secret);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_hmac_off_what_an_operator_needs_to_clean_up_still_works() {
    let _guard = TEST_MUTEX.lock().await;
    let deployment = deployment(HmacWebhooks::Disabled).await;
    let developer = deployment.developer().await;
    let left_behind = deployment.hmac_endpoint().await;

    // A sealed endpoint can still be created.
    let created = developer
        .request("POST", "/webhook_endpoints")
        .json(&create_body(&deployment.app_id, "sealed"))
        .send()
        .await;
    assert_eq!(created.status, 201, "{}", created.text());

    // The endpoint left behind can be deactivated, which touches neither the
    // mode nor a secret.
    let etag = etag_of(&developer, &left_behind.id).await;
    let deactivated = developer
        .request("PATCH", "/webhook_endpoints/{id}")
        .params(&[&left_behind.id])
        .if_match(&etag)
        .json(&serde_json::json!({"active": false}))
        .send()
        .await;
    assert_eq!(deactivated.status, 200, "{}", deactivated.text());
    assert!(!deployment.reread(&left_behind.id).await.active);

    // And switched to sealed, the way out.
    let switched = developer
        .request("POST", "/$procs/switchWebhookDeliveryMode")
        .json(&serde_json::json!({"args": {
            "endpointId": left_behind.id,
            "mode": "sealed",
            "audience": format!("moved-{}", unique_suffix()),
        }}))
        .send()
        .await;
    assert_eq!(switched.status, 200, "{}", switched.text());
    let after = deployment.reread(&left_behind.id).await;
    assert_eq!(after.deliveryMode, WebhookDeliveryMode::sealed);
    assert_eq!(after.secret, None);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_hmac_off_switching_to_hmac_and_rotating_are_422_and_write_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let deployment = deployment(HmacWebhooks::Disabled).await;
    let developer = deployment.developer().await;
    let sealed = deployment.sealed_endpoint().await;
    let hmac = deployment.hmac_endpoint().await;

    let switched = developer
        .request("POST", "/$procs/switchWebhookDeliveryMode")
        .json(&serde_json::json!({"args": {"endpointId": sealed.id, "mode": "hmac"}}))
        .send()
        .await;
    assert_disabled(&switched, "switchWebhookDeliveryMode to hmac");
    let unchanged = deployment.reread(&sealed.id).await;
    assert_eq!(unchanged.deliveryMode, WebhookDeliveryMode::sealed);
    assert_eq!(unchanged.version, sealed.version);

    let rotated = developer
        .request("POST", "/$procs/rotateWebhookSecret")
        .json(&serde_json::json!({"args": {"endpointId": hmac.id}}))
        .send()
        .await;
    assert_disabled(&rotated, "rotateWebhookSecret");
    let unchanged = deployment.reread(&hmac.id).await;
    assert_eq!(unchanged.secret, hmac.secret, "no secret was minted");
    assert_eq!(unchanged.prevSecret, None);
    assert_eq!(unchanged.version, hmac.version);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_hmac_off_a_caller_without_webhook_manage_is_told_so_first() {
    // The gate sits under the RBAC layers: a scope-less client hears about its
    // missing permission, not about the deployment's methods.
    let _guard = TEST_MUTEX.lock().await;
    let deployment = deployment(HmacWebhooks::Disabled).await;
    let row = {
        let suffix = unique_suffix();
        deployment
            .db
            .app_client()
            .create(schema::CreateAppClientInput {
                appId: deployment.app_id.clone(),
                clientId: format!("appc_gate_{suffix}"),
                label: "hmac gate test client".to_owned(),
                scopes: " ".to_owned(),
                lastUsedAt: None,
                retiredAt: None,
            })
            .run(&system_context("hmac-gate-test"))
            .await
            .expect("seeding the app client")
            .id
    };
    let key = TestKey::app();
    register_app_key(&deployment.db, &row, &key).await;
    let api = client(&deployment.base, &key);

    let reply = api
        .request("POST", "/webhook_endpoints")
        .json(&create_body(&deployment.app_id, "hmac"))
        .send()
        .await;
    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(
        message_of(&reply).contains("webhook:manage"),
        "{}",
        reply.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_hmac_permitted_every_call_is_what_it_was() {
    let _guard = TEST_MUTEX.lock().await;
    let deployment = deployment(HmacWebhooks::Permitted).await;
    let developer = deployment.developer().await;
    let sealed = deployment.sealed_endpoint().await;
    let hmac = deployment.hmac_endpoint().await;

    let created = developer
        .request("POST", "/webhook_endpoints")
        .json(&create_body(&deployment.app_id, "hmac"))
        .send()
        .await;
    assert_eq!(created.status, 201, "{}", created.text());

    let etag = etag_of(&developer, &hmac.id).await;
    let updated = developer
        .request("PATCH", "/webhook_endpoints/{id}")
        .params(&[&hmac.id])
        .if_match(&etag)
        .json(&serde_json::json!({"secret": format!("{}2", key_of_32())}))
        .send()
        .await;
    assert_eq!(updated.status, 200, "{}", updated.text());

    let rotated = developer
        .request("POST", "/$procs/rotateWebhookSecret")
        .json(&serde_json::json!({"args": {"endpointId": hmac.id}}))
        .send()
        .await;
    assert_eq!(rotated.status, 200, "{}", rotated.text());
    assert!(
        rotated.json()["prevSecret"].as_str().is_some(),
        "the old secret moved to prevSecret: {}",
        rotated.text()
    );

    let switched = developer
        .request("POST", "/$procs/switchWebhookDeliveryMode")
        .json(&serde_json::json!({"args": {"endpointId": sealed.id, "mode": "hmac"}}))
        .send()
        .await;
    assert_eq!(switched.status, 200, "{}", switched.text());
    assert_eq!(
        deployment.reread(&sealed.id).await.deliveryMode,
        WebhookDeliveryMode::hmac
    );
}
