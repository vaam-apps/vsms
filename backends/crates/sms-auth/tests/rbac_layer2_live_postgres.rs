//! The live gate for #24: Layer 2 (permission/scope enforcement, §5.1 of
//! the design doc) checked over a *real* HTTP round trip - a real signing key
//! registered for a real `AppClient` (or paired to a real `User`), a real
//! sealed (`COSE_Sign1`) request, `sms_api::router`'s own `require_permission`/
//! `enforce_route_permission` deciding on the verified signer's scopes or
//! role permissions, against a real Postgres. Not a hand-built
//! `CratestackContext`: see `backends/crates/sms-api/tests/errors_live_postgres.rs`'s
//! own history (#87) for why this repo doesn't trust a green `cargo test`
//! that never went through the real identity-extraction path, and #29's own
//! `claim_live_postgres.rs` for the same lesson landing a second time
//! (silently-zero-rows policy gaps that only a real query ever surfaces).
//!
//! Since VS-3 there is no token to vary. What this suite used to do by
//! requesting a narrower `scope` from `/token` it does by registering a key
//! for an `AppClient` that *holds* a narrower scope: none, a different one, or
//! the right one. The scope is the client's, so each case has its own client.
//!
//! Anchors:
//!
//! - **`sendMessage`** (`sms:send`), gated by `Procedures::send`'s own
//!   `require_permission` call: no scope is denied, a scope that doesn't
//!   contain `sms:send` is denied, and one that does succeeds end to end (a
//!   real `Message` row lands in Postgres).
//!
//! - **`PATCH /providers/{id}`** (`provider:update`), `sms-api`'s concrete
//!   anchor for #25 (`router::PROVIDER_WRITE_ROUTES`). An app key is denied
//!   (no scope, and separately a working `sms:send` scope that simply isn't
//!   the permission this route wants). **Unlike before VS-3, the success case
//!   is provable here**: a paired console key makes an `operator` a real
//!   `user` principal whose role grants `provider:update`, so the same route
//!   answers 200 to that person, and refuses a `developer`, whose role grants
//!   nothing `provider:*`.
//!
//! - **`GET /jobs`** (`job:read`), #56/#57's own anchor
//!   (`router::JOB_READ_ROUTES`): no scope, wrong scope, and the actual
//!   success case, because `Job`'s Layer 1 `@@allow` admits `kind == "app"`.
//!
//! - **`POST /sender_ids`** (`sender:manage`) and **`POST /webhook_endpoints`**
//!   (`webhook:manage`), #53/#55's anchors: Layer 2 names the missing
//!   permission before Layer 1 is asked.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! `sms_test_support` provisions Postgres and applies every migration
//! automatically (a shared, self-healing container - see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-auth --test rbac_layer2_live_postgres -- --ignored
//! ```

#[path = "../../sms-api/tests/support/sealed_fixtures.rs"]
mod sealed_fixtures;

use chrono::Utc;
use cratestack::CratestackContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use sealed_fixtures::{
    api_router, client, owner, register_app_key, seed_user_with_console_key, serve,
};
use sms_api::auth::system_context;
use sms_api::schema::{self, Cratestack, SenderIdKind, SenderIdRegistrationStatus};
use sms_test_support::sealed::{Reply, SealedClient, TestKey};

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests - run concurrently by Rust's default multi-threaded test
/// harness - can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape at the
/// same instant. See `backends/crates/sms-worker/tests/claim_live_postgres.rs`'s
/// own `TEST_MUTEX` doc for the full reasoning - same mechanism, same fix.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CratestackContext {
    system_context("rbac-layer2-test-system")
}

fn unique_suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

/// A fresh MSISDN under the seeded `67x` (mtn) prefix - see
/// `backends/crates/sms-api/tests/send_message_live_postgres.rs`'s own copy of
/// this helper for why it needs cross-run, not just cross-call, uniqueness.
fn unique_mtn_msisdn() -> String {
    let n = u128::from_le_bytes(*cratestack::uuid::Uuid::new_v4().as_bytes()) % 1_000_000;
    format!("+237677{n:06}")
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

/// An `App` and an active `AppClient` holding exactly `scopes`; returns the
/// client's row id (what a `ClientKey` points at).
async fn seed_app_client(db: &Cratestack, scopes: &str) -> String {
    let suffix = unique_suffix();
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "rbac layer2 test app".to_owned(),
            slug: format!("rbac-layer2-test-{suffix}"),
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

    db.app_client()
        .create(schema::CreateAppClientInput {
            appId: app.id,
            clientId: format!("appc_rbac_{suffix}"),
            label: "rbac layer2 test client".to_owned(),
            scopes: scopes.to_owned(),
            lastUsedAt: None,
            retiredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the app client")
        .id
}

/// An active `SenderId` with an `approved` registration against a fresh
/// `Provider` row - `sendMessage` needs the former to accept a send at
/// all, and this suite's `provider:update` route assertions need a real,
/// existing row to `PATCH` against rather than a made-up id (a 403 on a
/// row that actually exists is the meaningful assertion; a 403 that could
/// just as easily be a 404 in disguise is not). Returns `(senderIdValue,
/// providerId)`.
async fn seed_sender_and_provider(db: &Cratestack) -> (String, String) {
    let suffix = unique_suffix();
    let value = format!("T{}", &suffix[..9]).to_uppercase();

    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!("test_{}", suffix.to_lowercase()),
            displayName: "RBAC Layer 2 Test Provider".to_owned(),
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

    let sender = db
        .sender_id()
        .create(schema::CreateSenderIdInput {
            value: value.clone(),
            kind: SenderIdKind::alphanumeric,
            notes: None,
        })
        .run(&owner())
        .await
        .expect("seeding a sender id");

    db.sender_id_registration()
        .create(schema::CreateSenderIdRegistrationInput {
            senderIdId: sender.id.clone(),
            providerId: provider.id.clone(),
            status: SenderIdRegistrationStatus::approved,
            submittedAt: Some(Utc::now()),
            approvedAt: Some(Utc::now()),
            reference: None,
            rejectionReason: None,
        })
        .run(&owner())
        .await
        .expect("seeding an approved registration");

    db.sender_id()
        .update(sender.id)
        .set(schema::UpdateSenderIdInput {
            active: Some(true),
            ..Default::default()
        })
        // #59: SenderId is now @version'd.
        .if_match(sender.version)
        .run(&owner())
        .await
        .expect("activating the sender id");

    (value, provider.id)
}

struct TestServer {
    db: Cratestack,
    base: String,
    sender_id: String,
    provider_id: String,
}

async fn setup() -> TestServer {
    let db = db().await;
    // #134: this suite exercises Layer 2 (`require_permission`), so any
    // pepper over the minimum length works.
    let base = serve(api_router(
        &db,
        "rbac-layer2-live-test-pepper-well-over-the-minimum-length",
    ))
    .await;
    let (sender_id, provider_id) = seed_sender_and_provider(&db).await;
    TestServer {
        db,
        base,
        sender_id,
        provider_id,
    }
}

impl TestServer {
    /// A sealed client for a fresh `AppClient` that holds exactly `scopes`.
    async fn app_with(&self, scopes: &str) -> SealedClient {
        let row = seed_app_client(&self.db, scopes).await;
        let key = TestKey::app();
        register_app_key(&self.db, &row, &key).await;
        client(&self.base, &key)
    }

    /// A sealed client for a fresh user in `role_key`, paired to a fresh key.
    async fn person_in(&self, role_key: &str) -> SealedClient {
        let key = TestKey::console();
        seed_user_with_console_key(&self.db, role_key, &key).await;
        client(&self.base, &key)
    }

    fn send_body(&self, text: &str) -> serde_json::Value {
        serde_json::json!({"args": {
            "to": unique_mtn_msisdn(),
            "body": text,
            "senderId": self.sender_id,
        }})
    }
}

fn message_of(reply: &Reply) -> String {
    reply.json()["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn send_message_denies_a_client_with_no_scope_at_all() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" ").await;

    let reply = api
        .request("POST", "/$procs/sendMessage")
        .json(&server.send_body("should never send"))
        .send()
        .await;

    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(
        message_of(&reply).contains("sms:send"),
        "expected the denial to name the missing permission: {}",
        reply.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn send_message_denies_a_client_scoped_for_something_else() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" sms:read ").await;

    let reply = api
        .request("POST", "/$procs/sendMessage")
        .json(&server.send_body("should never send"))
        .send()
        .await;

    assert_eq!(
        reply.status,
        403,
        "a client scoped only for sms:read must not be able to send: {}",
        reply.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn send_message_succeeds_for_a_client_carrying_sms_send() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" sms:send ").await;

    let reply = api
        .request("POST", "/$procs/sendMessage")
        .json(&server.send_body("a real send, gated by a real sms:send scope"))
        .send()
        .await;

    assert!(
        (200..300).contains(&reply.status),
        "a correctly-scoped client must be able to send: {} {}",
        reply.status,
        reply.text()
    );
    assert!(
        reply.json()["messageId"].as_str().is_some(),
        "a successful send returns the persisted message's id: {}",
        reply.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn provider_write_route_rejects_an_unsigned_request() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;

    let response = reqwest::Client::new()
        .patch(format!("{}/providers/{}", server.base, server.provider_id))
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("calling PATCH /providers/{id} unsigned");

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn provider_write_route_denies_a_client_with_no_scope_at_all() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" ").await;

    let reply = api
        .request("PATCH", "/providers/{id}")
        .params(&[&server.provider_id])
        .json(&serde_json::json!({"maxTps": 10.0}))
        .send()
        .await;

    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(
        message_of(&reply).contains("provider:update"),
        "expected the denial to name the missing permission: {}",
        reply.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn provider_write_route_denies_a_client_that_is_correctly_scoped_for_something_else() {
    // The point of this test: a client this suite already proved *works*
    // for sendMessage - a real, held `sms:send` scope, not a missing one -
    // still cannot touch `PATCH /providers/{id}`. Layer 2 narrows; it never
    // widens (§5.1), and `sms:send` granting access to `sendMessage` must
    // never spill over into granting anything else.
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" sms:send ").await;

    let reply = api
        .request("PATCH", "/providers/{id}")
        .params(&[&server.provider_id])
        .json(&serde_json::json!({"maxTps": 10.0}))
        .send()
        .await;

    assert_eq!(reply.status, 403, "{}", reply.text());
}

/// The success case this suite could not prove before VS-3 (no token it
/// could mint carried a role): a paired `operator`, whose role grants
/// `provider:update`, clears Layer 2 and Layer 1 and the write lands. A
/// `developer`, whose role grants no `provider:*`, is refused by Layer 2
/// alone, naming the permission.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn provider_write_route_admits_an_operator_and_refuses_a_developer() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let operator = server.person_in("operator").await;
    let developer = server.person_in("developer").await;

    let refused = developer
        .request("PATCH", "/providers/{id}")
        .params(&[&server.provider_id])
        .json(&serde_json::json!({"maxTps": 10.0}))
        .send()
        .await;
    assert_eq!(refused.status, 403, "{}", refused.text());
    assert!(
        message_of(&refused).contains("provider:update"),
        "a developer is stopped by Layer 2, which names the permission: {}",
        refused.text()
    );

    let provider = operator
        .request("GET", "/providers/{id}")
        .params(&[&server.provider_id])
        .send()
        .await;
    assert_eq!(provider.status, 200, "{}", provider.text());
    let etag = provider
        .headers
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .expect("a versioned model answers with an ETag")
        .to_owned();

    let updated = operator
        .request("PATCH", "/providers/{id}")
        .params(&[&server.provider_id])
        .if_match(&etag)
        .json(&serde_json::json!({"maxTps": 10.0}))
        .send()
        .await;
    assert_eq!(updated.status, 200, "{}", updated.text());
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_route_this_middleware_does_not_gate_is_unaffected() {
    // `enforce_route_permission` is a no-op for any request that doesn't
    // match one of its route sets - a scope-less client must still be able to
    // reach `previewMessage` (`@allow(auth() != null)`, no permission
    // requirement at all) exactly as it could before #24.
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" ").await;

    let reply = api
        .request("POST", "/$procs/previewMessage")
        .json(&serde_json::json!({"args": {"body": "hello"}}))
        .send()
        .await;

    assert_eq!(
        reply.status,
        200,
        "previewMessage carries no permission requirement and must be unaffected by #24: {}",
        reply.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn job_read_route_denies_a_client_with_no_scope_at_all() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" ").await;

    let reply = api.request("GET", "/jobs").send().await;

    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(
        message_of(&reply).contains("job:read"),
        "expected the denial to name the missing permission: {}",
        reply.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn job_read_route_denies_a_client_scoped_for_something_else() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" sms:send ").await;

    let reply = api.request("GET", "/jobs").send().await;

    assert_eq!(
        reply.status,
        403,
        "a client scoped only for sms:send must not be able to read the job backlog: {}",
        reply.text()
    );
}

/// The success case `PROVIDER_WRITE_ROUTES`'s app-key tests above cannot
/// prove: `Job`'s Layer 1 `@@allow` genuinely admits `auth().kind == "app"`,
/// so a client holding `job:read` reaches all the way through to a real
/// `200`, not just past Layer 2's own gate.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn job_read_route_succeeds_for_a_client_carrying_job_read() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" job:read ").await;

    let reply = api.request("GET", "/jobs").send().await;

    assert_eq!(
        reply.status,
        200,
        "a client holding job:read must be able to read the job backlog: {}",
        reply.text()
    );
    assert!(
        reply.json()["items"].is_array(),
        "GET /jobs must return the standard paged envelope: {}",
        reply.text()
    );
}

/// #53/#55's own anchor - `router::SENDER_AND_WEBHOOK_WRITE_ROUTES`. An app
/// key is refused by Layer 2 when it holds no `sender:manage`, and the denial
/// names the permission.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn sender_write_route_denies_a_client_with_no_scope_at_all() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" ").await;

    let reply = api
        .request("POST", "/sender_ids")
        .json(&serde_json::json!({"value": unique_suffix(), "kind": "alphanumeric"}))
        .send()
        .await;

    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(
        message_of(&reply).contains("sender:manage"),
        "expected the denial to name the missing permission: {}",
        reply.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn sender_write_route_denies_a_client_that_is_correctly_scoped_for_something_else() {
    // Same point as `provider_write_route_denies_a_client_that_is_correctly_
    // scoped_for_something_else`: a client this suite already proved *works*
    // for sendMessage must not spill over into granting `sender:manage`.
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" sms:send ").await;

    let reply = api
        .request("POST", "/sender_ids")
        .json(&serde_json::json!({"value": unique_suffix(), "kind": "alphanumeric"}))
        .send()
        .await;

    assert_eq!(reply.status, 403, "{}", reply.text());
}

/// The webhook-endpoint half of the same anchor, `webhook:manage` rather
/// than `sender:manage`.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn webhook_write_route_denies_a_client_with_no_scope_at_all() {
    let _guard = TEST_MUTEX.lock().await;
    let server = setup().await;
    let api = server.app_with(" ").await;

    let reply = api
        .request("POST", "/webhook_endpoints")
        .json(&serde_json::json!({
            "appId": "does-not-matter-layer2-runs-first",
            "url": "https://example.test/hooks",
            "eventTypes": " message.delivered ",
            "secret": format!("whsec_test_{}", unique_suffix()),
            "maskRecipient": true,
            "maxAttempts": 8,
        }))
        .send()
        .await;

    assert_eq!(reply.status, 403, "{}", reply.text());
    assert!(
        message_of(&reply).contains("webhook:manage"),
        "expected the denial to name the missing permission: {}",
        reply.text()
    );
}
