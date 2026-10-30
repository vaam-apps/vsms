//! The signed transport (VS-3) over a real HTTP round trip against a real
//! Postgres: the envelope layer is `Required` for every generated route, keys
//! come from `ClientKey` / `ConsoleKey`, replays are caught by
//! `EnvelopeNonce`, and the Layer 2 guards read the verified signer.
//!
//! Every test signs with a throwaway key from `sms_test_support::sealed` and
//! registers its public half through `support/sealed_fixtures.rs`. Nothing
//! here mints a token; there is none to mint.
//!
//! Ignored by default, same convention as this workspace's other live suites:
//!
//! ```bash
//! cargo test -p sms-api --test envelope_live_postgres -- --ignored
//! ```

#[path = "support/sealed_fixtures.rs"]
mod sealed_fixtures;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use cratestack::cose::{CoseAlg, CoseVerifierResolver as _};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, CratestackError, FilterExpr, NonceStore as _, VerifiedSigner};
use sealed_fixtures::{
    api_router, client, deactivate_app_client, deactivate_user, owner, pair_console_key,
    register_app_key, seed_user_with_console_key, serve,
};
use sms_api::auth::system_context;
use sms_api::envelope::PgNonceStore;
use sms_api::schema::{self, Cratestack, SenderIdKind, SenderIdRegistrationStatus, message};
use sms_api::signers::{DirectoryKeys, SignerDirectory, Standing};
use sms_test_support::sealed::TestKey;

const PEPPER: &str = "envelope-live-test-pepper-well-over-the-minimum-length";

/// What the envelope layer says to anything it will not open, byte for byte.
const COARSE_REFUSAL: &str =
    r#"{"code":"UNAUTHORIZED","message":"request could not be authenticated","details":null}"#;

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CratestackContext {
    system_context("envelope-live-test")
}

fn suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

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

/// An `App` and an `AppClient` holding `scopes`; returns the client's row id,
/// its `clientId` and the app's id.
async fn seed_app_client(db: &Cratestack, scopes: &str) -> (String, String, String) {
    let tag = suffix();
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "envelope live test app".to_owned(),
            slug: format!("envelope-live-{tag}"),
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
    let client_id = format!("appc_envelope_{tag}");
    let row = db
        .app_client()
        .create(schema::CreateAppClientInput {
            appId: app.id.clone(),
            clientId: client_id.clone(),
            label: "envelope live test client".to_owned(),
            scopes: scopes.to_owned(),
            lastUsedAt: None,
            retiredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the app client");
    (row.id, client_id, app.id)
}

/// An active sender with an approved registration, so `sendMessage` accepts a
/// send. Returns the sender id value.
async fn seed_sender(db: &Cratestack) -> String {
    let tag = suffix();
    let value = format!("E{}", &tag[..8]).to_uppercase();
    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!("envelope_{tag}"),
            displayName: "Envelope Live Test Provider".to_owned(),
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
            providerId: provider.id,
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
        .if_match(sender.version)
        .run(&owner())
        .await
        .expect("activating the sender id");
    value
}

struct Rig {
    db: Cratestack,
    base: String,
    sender: String,
}

async fn rig() -> Rig {
    let db = db().await;
    let sender = seed_sender(&db).await;
    let base = serve(api_router(&db, PEPPER)).await;
    Rig { db, base, sender }
}

fn send_body(rig: &Rig, client_ref: &str) -> serde_json::Value {
    serde_json::json!({"args": {
        "to": unique_mtn_msisdn(),
        "body": "a sealed send",
        "senderId": rig.sender,
        "clientRef": client_ref,
    }})
}

async fn messages_with_client_ref(db: &Cratestack, app_id: &str, client_ref: &str) -> usize {
    db.message()
        .find_many()
        .where_expr(
            FilterExpr::from(message::appId().eq(app_id.to_owned()))
                .and(message::clientRef().eq(client_ref.to_owned())),
        )
        .run(&sys())
        .await
        .expect("reading messages back")
        .len()
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_sealed_send_message_from_an_app_key_succeeds() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (row_id, _client_id, app_id) = seed_app_client(&rig.db, " sms:send ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &row_id, &key).await;
    let client_ref = format!("sealed-{}", suffix());

    let reply = client(&rig.base, &key)
        .request("POST", "/$procs/sendMessage")
        .json(&send_body(&rig, &client_ref))
        .send()
        .await;

    assert!(reply.sealed, "the answer is sealed: {}", reply.text());
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(reply.json()["messageId"].is_string(), "{}", reply.text());
    assert_eq!(
        messages_with_client_ref(&rig.db, &app_id, &client_ref).await,
        1
    );
}

/// A signed `GET` has no payload and is sealed all the same (cratestack
/// decision D3), and a procedure that needs no permission reaches a result.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_signed_get_and_a_permissionless_procedure_both_work() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (row_id, ..) = seed_app_client(&rig.db, " job:read ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &row_id, &key).await;
    let api = client(&rig.base, &key);

    let jobs = api.request("GET", "/jobs").send().await;
    assert!(jobs.sealed && jobs.status == 200, "{}", jobs.text());
    assert!(jobs.json()["items"].is_array(), "{}", jobs.text());

    let preview = api
        .request("POST", "/$procs/previewMessage")
        .json(&serde_json::json!({"args": {"body": "hello"}}))
        .send()
        .await;
    assert!(
        preview.sealed && preview.status == 200,
        "{}",
        preview.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_sealed_admin_call_works_and_the_owner_grant_guard_still_refuses_an_admin_granting_owner()
{
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let admin_key = TestKey::console();
    let admin = seed_user_with_console_key(&rig.db, "admin", &admin_key).await;
    let api = client(&rig.base, &admin_key);

    // The admin's own sealed read works: this is a user principal now.
    let roles = api.request("GET", "/roles").send().await;
    assert!(roles.sealed && roles.status == 200, "{}", roles.text());

    // The incident, sealed: an admin PATCHes its own record to `owner`.
    let to_owner = api
        .request("PATCH", "/users/{id}")
        .params(&[&admin.id])
        .if_match(&format!("\"{}\"", admin.version))
        .json(&serde_json::json!({"roleKey": "owner"}))
        .send()
        .await;
    assert!(to_owner.sealed, "{}", to_owner.text());
    assert_eq!(to_owner.status, 403, "{}", to_owner.text());
    let after = rig
        .db
        .user()
        .find_unique(admin.id.clone())
        .run(&owner())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.roleKey, "admin", "the row is unchanged");

    // And creating a new owner is refused the same way.
    let create_owner = api
        .request("POST", "/users")
        .json(&serde_json::json!({
            "subject": format!("sub-{}", suffix()),
            "email": format!("{}@fixture.invalid", suffix()),
            "displayName": "Would-be owner",
            "roleKey": "owner",
        }))
        .send()
        .await;
    assert!(create_owner.sealed);
    assert_eq!(create_owner.status, 403, "{}", create_owner.text());

    // The guard is about the value written, not about the route: an admin may
    // still create an `operator`.
    let create_operator = api
        .request("POST", "/users")
        .json(&serde_json::json!({
            "subject": format!("sub-{}", suffix()),
            "email": format!("{}@fixture.invalid", suffix()),
            "displayName": "An operator",
            "roleKey": "operator",
        }))
        .send()
        .await;
    assert!(
        create_operator.status == 200 || create_operator.status == 201,
        "{} {}",
        create_operator.status,
        create_operator.text()
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_owner_may_still_grant_owner_when_sealed() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let owner_key = TestKey::console();
    seed_user_with_console_key(&rig.db, "owner", &owner_key).await;

    let created = client(&rig.base, &owner_key)
        .request("POST", "/users")
        .json(&serde_json::json!({
            "subject": format!("sub-{}", suffix()),
            "email": format!("{}@fixture.invalid", suffix()),
            "displayName": "A second owner",
            "roleKey": "owner",
        }))
        .send()
        .await;
    assert!(
        created.status == 200 || created.status == 201,
        "{} {}",
        created.status,
        created.text()
    );
}

/// Unsigned means 401 and nothing runs: no row is written by an unsigned
/// `sendMessage`, even one that carries a well-formed bearer.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_unsigned_request_is_a_401_and_runs_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (row_id, _client_id, app_id) = seed_app_client(&rig.db, " sms:send ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &row_id, &key).await;
    let client_ref = format!("unsigned-{}", suffix());
    let http = reqwest::Client::new();

    for bearer in [None, Some("Bearer not-a-token")] {
        let mut request = http
            .post(format!("{}/$procs/sendMessage", rig.base))
            .header("accept", "application/json")
            .json(&send_body(&rig, &client_ref));
        if let Some(bearer) = bearer {
            request = request.header("authorization", bearer);
        }
        let response = request.send().await.expect("sending");
        assert_eq!(response.status().as_u16(), 401, "bearer: {bearer:?}");
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        assert!(
            !content_type.starts_with("application/cose"),
            "the refusal is unsigned"
        );
        // The envelope layer's own coarse answer, not the inner auth provider's:
        // with `EnvelopeMode::Optional` an unsigned request would reach
        // `GatewayAuth`, which also says 401 but in its own words (and 400 for a
        // bearer), so the status alone cannot tell the two modes apart.
        let body = response.text().await.expect("reading the refusal");
        assert_eq!(
            body, COARSE_REFUSAL,
            "bearer {bearer:?}: the envelope refused it itself"
        );
    }
    assert_eq!(
        messages_with_client_ref(&rig.db, &app_id, &client_ref).await,
        0
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_unregistered_key_is_a_401() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let stranger = TestKey::app();

    let reply = client(&rig.base, &stranger)
        .request("POST", "/$procs/previewMessage")
        .json(&serde_json::json!({"args": {"body": "hello"}}))
        .send()
        .await;

    assert_eq!(reply.status, 401);
    assert!(!reply.sealed, "an unknown key earns no signed answer");
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_revoked_client_key_is_a_401() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (row_id, ..) = seed_app_client(&rig.db, " sms:send ").await;
    let key = TestKey::app();
    let registered = register_app_key(&rig.db, &row_id, &key).await;
    let api = client(&rig.base, &key);
    let preview = || {
        api.request("POST", "/$procs/previewMessage")
            .json(&serde_json::json!({"args": {"body": "hello"}}))
            .send()
    };

    let before = preview().await;
    assert!(before.sealed && before.status == 200, "{}", before.text());

    rig.db
        .client_key()
        .update(registered.id)
        .set(schema::UpdateClientKeyInput {
            revokedAt: Some(Some(Utc::now())),
            ..Default::default()
        })
        .run(&sys())
        .await
        .expect("revoking the key");

    let after = preview().await;
    assert_eq!(after.status, 401, "{}", after.text());
    assert!(!after.sealed);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_revoked_console_key_is_a_401() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let key = TestKey::console();
    let user = seed_user_with_console_key(&rig.db, "admin", &key).await;
    let api = client(&rig.base, &key);

    let before = api.request("GET", "/roles").send().await;
    assert!(before.sealed && before.status == 200, "{}", before.text());

    let paired = rig
        .db
        .console_key()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::console_key::userId().eq(user.id.clone()),
        ))
        .run(&sys())
        .await
        .unwrap()
        .remove(0);
    rig.db
        .console_key()
        .update(paired.id)
        .set(schema::UpdateConsoleKeyInput {
            revokedAt: Some(Some(Utc::now())),
            ..Default::default()
        })
        .run(&sys())
        .await
        .expect("revoking the key");

    let after = api.request("GET", "/roles").send().await;
    assert_eq!(after.status, 401, "{}", after.text());
    assert!(!after.sealed);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_expired_console_key_is_a_401() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let key = TestKey::console();
    let user = seed_user_with_console_key(&rig.db, "admin", &TestKey::console()).await;
    // The same person, a second key that has already expired.
    pair_console_key(&rig.db, &user.id, &key, Duration::seconds(-30)).await;

    let reply = client(&rig.base, &key)
        .request("GET", "/roles")
        .send()
        .await;

    assert_eq!(reply.status, 401, "{}", reply.text());
    assert!(!reply.sealed);
}

/// A good signature from a client or a person who may no longer act is the
/// same unsigned 401 as a forgery: the principal mapper refuses them. (Each
/// router is built after the deactivation, because the standing of an owner is
/// cached for a minute.)
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_inactive_client_or_person_is_a_401() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let (row_id, ..) = seed_app_client(&db, " sms:send ").await;
    let app_key = TestKey::app();
    register_app_key(&db, &row_id, &app_key).await;
    deactivate_app_client(&db, &row_id).await;

    let person_key = TestKey::console();
    let person = seed_user_with_console_key(&db, "admin", &person_key).await;
    deactivate_user(&db, &person.id).await;

    let base = serve(api_router(&db, PEPPER)).await;
    let app_reply = client(&base, &app_key)
        .request("POST", "/$procs/previewMessage")
        .json(&serde_json::json!({"args": {"body": "hello"}}))
        .send()
        .await;
    assert_eq!(app_reply.status, 401, "{}", app_reply.text());
    assert!(!app_reply.sealed);

    let person_reply = client(&base, &person_key)
        .request("GET", "/roles")
        .send()
        .await;
    assert_eq!(person_reply.status, 401, "{}", person_reply.text());
    assert!(!person_reply.sealed);
}

/// The nonce store, end to end: the same sealed bytes delivered twice run
/// once. The second delivery is the coarse unsigned 401 (never a signed one,
/// for a request that already executed) and writes nothing.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_replayed_request_is_refused_and_runs_once() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (row_id, _client_id, app_id) = seed_app_client(&rig.db, " sms:send ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &row_id, &key).await;
    let client_ref = format!("replay-{}", suffix());

    let prepared = client(&rig.base, &key)
        .request("POST", "/$procs/sendMessage")
        .json(&send_body(&rig, &client_ref))
        .seal()
        .await;

    let first = prepared.resend().await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert!(first.sealed);
    let second = prepared.resend().await;
    assert_eq!(second.status, 401, "{}", second.text());
    assert!(!second.sealed, "a replay earns no signed answer");

    assert_eq!(
        messages_with_client_ref(&rig.db, &app_id, &client_ref).await,
        1
    );
}

/// A client that re-seals its retry (a new `cti`) under the same
/// `Idempotency-Key` gets the stored response, not a second execution: the
/// idempotency layer sits inside the envelope and keys on the verified
/// principal.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_resealed_retry_under_one_idempotency_key_replays_the_first_answer() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (row_id, _client_id, app_id) = seed_app_client(&rig.db, " sms:send ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &row_id, &key).await;
    let client_ref = format!("idem-{}", suffix());
    let idem = format!("idem-key-{}", suffix());
    let body = send_body(&rig, &client_ref);
    let api = client(&rig.base, &key);

    let first = api
        .request("POST", "/$procs/sendMessage")
        .idempotency_key(&idem)
        .json(&body)
        .send()
        .await;
    let second = api
        .request("POST", "/$procs/sendMessage")
        .idempotency_key(&idem)
        .json(&body)
        .send()
        .await;

    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(second.status, 200, "{}", second.text());
    assert_eq!(first.json()["messageId"], second.json()["messageId"]);
    assert_eq!(
        second
            .headers
            .get("idempotency-replayed")
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );
    assert_eq!(
        messages_with_client_ref(&rig.db, &app_id, &client_ref).await,
        1
    );
}

/// Nothing re-reads an `Authorization` header: beside a good signature it is
/// refused outright, so a stale client cannot believe a bearer still counts.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_bearer_on_a_sealed_route_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (row_id, _client_id, app_id) = seed_app_client(&rig.db, " sms:send ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &row_id, &key).await;
    let client_ref = format!("bearer-{}", suffix());

    let reply = client(&rig.base, &key)
        .request("POST", "/$procs/sendMessage")
        .header("authorization", "Bearer left-over-from-the-token-days")
        .json(&send_body(&rig, &client_ref))
        .send()
        .await;

    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(
        reply.text().contains("bearer credentials are not accepted"),
        "{}",
        reply.text()
    );
    assert_eq!(
        messages_with_client_ref(&rig.db, &app_id, &client_ref).await,
        0
    );
}

/// The router's codec is JSON, so JSON is the one sealed payload type. A
/// correctly signed CBOR request is refused before any key is looked up, with
/// the layer's unsigned `415`, instead of opening fine and failing to decode.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_cbor_payload_is_an_unsigned_415_and_runs_nothing() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (row_id, _client_id, app_id) = seed_app_client(&rig.db, " sms:send ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &row_id, &key).await;
    let client_ref = format!("cbor-{}", suffix());
    let body = serde_json::to_vec(&send_body(&rig, &client_ref)).unwrap();

    let reply = client(&rig.base, &key)
        .request("POST", "/$procs/sendMessage")
        .payload(&body, "application/cbor")
        .send()
        .await;

    assert!(!reply.sealed, "a refusal on the payload type is unsigned");
    assert_eq!(reply.status, 415, "{}", reply.text());
    assert_eq!(
        messages_with_client_ref(&rig.db, &app_id, &client_ref).await,
        0
    );
}

/// A key's authority is its owner's: an app client signs as an app (scopes, no
/// role), so the human-only routes stay closed to it at Layer 1.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_app_key_cannot_reach_the_human_only_routes() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (row_id, ..) = seed_app_client(&rig.db, " sms:send sender:manage provider:update ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &row_id, &key).await;

    let reply = client(&rig.base, &key)
        .request("POST", "/sender_ids")
        .json(&serde_json::json!({"value": format!("X{}", &suffix()[..6]), "kind": "alphanumeric"}))
        .send()
        .await;

    // Layer 2 passes (the scope is held); Layer 1's policy has no
    // `hasRole('app')`, so the write is refused.
    assert!(reply.sealed);
    assert_eq!(reply.status, 403, "{}", reply.text());
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_nonce_store_records_once_and_takes_over_an_expired_entry() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let store = PgNonceStore::new(db.clone());
    let nonce = format!("cose:test-{}", suffix());

    assert!(
        store
            .record_if_unseen(&nonce, Utc::now() + Duration::minutes(10))
            .await
            .unwrap(),
        "first sighting"
    );
    assert!(
        !store
            .record_if_unseen(&nonce, Utc::now() + Duration::minutes(10))
            .await
            .unwrap(),
        "second sighting inside the window"
    );

    // An entry whose window has closed is taken over, not refused forever.
    let stale = format!("cose:test-{}", suffix());
    assert!(
        store
            .record_if_unseen(&stale, Utc::now() - Duration::minutes(1))
            .await
            .unwrap()
    );
    assert!(
        store
            .record_if_unseen(&stale, Utc::now() + Duration::minutes(10))
            .await
            .unwrap(),
        "an expired entry is taken over"
    );
    assert!(
        !store
            .record_if_unseen(&stale, Utc::now() + Duration::minutes(10))
            .await
            .unwrap(),
        "and the new window holds"
    );
}

/// Eight deliveries of one nonce at once: the unique index lets exactly one
/// through, which is the whole point of the table.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn one_nonce_recorded_concurrently_is_unseen_exactly_once() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let store = std::sync::Arc::new(PgNonceStore::new(db));
    let nonce = format!("cose:race-{}", suffix());

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (store, nonce) = (store.clone(), nonce.clone());
        tasks.push(tokio::spawn(async move {
            store
                .record_if_unseen(&nonce, Utc::now() + Duration::minutes(10))
                .await
                .expect("recording")
        }));
    }
    let mut firsts = 0;
    for task in tasks {
        if task.await.unwrap() {
            firsts += 1;
        }
    }
    assert_eq!(firsts, 1, "exactly one delivery is a first sighting");
}

/// Reads happen under the system context and are not an error path: a
/// resolver miss is an empty list, so an unknown `kid` is a 401, never a 500.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_directory_refuses_an_unknown_thumbprint_as_unauthorized() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let directory = sms_api::signers::SignerDirectory::new(db);

    for alg in [CoseAlg::Ed25519, CoseAlg::Esp256] {
        let error = directory
            .standing(&signer_of_thumbprint(&[9; 32], alg), Utc::now())
            .await
            .expect_err("nothing is registered under this thumbprint");
        assert!(
            matches!(error, CratestackError::Unauthorized(_)),
            "{alg:?}: {error:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The two places a key is looked up, each tested alone.
//
// Revocation and expiry are checked twice: by the verifier-side resolver
// (`DirectoryKeys`, which stops a dead key from verifying at all) and by the
// directory (`SignerDirectory::standing`, which decides who a verified key is).
// Through the router either one is enough to give the unsigned 401, so a
// request-level test cannot tell which of the two checks it is exercising.
// These call each component directly, so loosening any one check fails exactly
// its own test.
// ---------------------------------------------------------------------------

fn thumbprint_of(key: &TestKey) -> [u8; 32] {
    let bytes = (0..32)
        .map(|i| u8::from_str_radix(&key.thumbprint_hex()[2 * i..2 * i + 2], 16).unwrap())
        .collect::<Vec<_>>();
    bytes.try_into().expect("a 32-byte thumbprint")
}

fn signer_of_thumbprint(thumbprint: &[u8; 32], alg: CoseAlg) -> VerifiedSigner {
    VerifiedSigner::new(thumbprint[..8].to_vec(), *thumbprint, alg.id())
}

fn signer_of(key: &TestKey, alg: CoseAlg) -> VerifiedSigner {
    signer_of_thumbprint(&thumbprint_of(key), alg)
}

async fn resolved(db: &Cratestack, key: &TestKey, alg: CoseAlg) -> usize {
    let thumbprint = thumbprint_of(key);
    DirectoryKeys::new(db.clone())
        .resolve(&thumbprint[..8], alg)
        .await
        .expect("the resolver reads under the system context")
        .len()
}

async fn revoke_client_key(db: &Cratestack, key_id: String) {
    db.client_key()
        .update(key_id)
        .set(schema::UpdateClientKeyInput {
            revokedAt: Some(Some(Utc::now())),
            ..Default::default()
        })
        .run(&sys())
        .await
        .expect("revoking the client key");
}

async fn revoke_console_key(db: &Cratestack, user_id: &str) {
    let paired = db
        .console_key()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::console_key::userId().eq(user_id.to_owned()),
        ))
        .run(&sys())
        .await
        .unwrap()
        .remove(0);
    db.console_key()
        .update(paired.id)
        .set(schema::UpdateConsoleKeyInput {
            revokedAt: Some(Some(Utc::now())),
            ..Default::default()
        })
        .run(&sys())
        .await
        .expect("revoking the console key");
}

async fn standing_is_refused(directory: &SignerDirectory, key: &TestKey, alg: CoseAlg) -> bool {
    matches!(
        directory.standing(&signer_of(key, alg), Utc::now()).await,
        Err(CratestackError::Unauthorized(_))
    )
}

/// The control for the tests below: live keys resolve and have a standing, so
/// a refusal elsewhere is the check's and not a fixture that never worked.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn live_keys_resolve_and_have_a_standing() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let directory = SignerDirectory::new(db.clone());
    let (row_id, client_id, _) = seed_app_client(&db, " sms:send ").await;
    let app_key = TestKey::app();
    register_app_key(&db, &row_id, &app_key).await;
    let console_key = TestKey::console();
    let user = seed_user_with_console_key(&db, "admin", &console_key).await;

    assert_eq!(resolved(&db, &app_key, CoseAlg::Ed25519).await, 1);
    assert_eq!(resolved(&db, &console_key, CoseAlg::Esp256).await, 1);
    let app = directory
        .standing(&signer_of(&app_key, CoseAlg::Ed25519), Utc::now())
        .await
        .expect("a live client key");
    assert!(matches!(app, Standing::App(app) if app.client_id == client_id));
    let person = directory
        .standing(&signer_of(&console_key, CoseAlg::Esp256), Utc::now())
        .await
        .expect("a live console key");
    assert!(matches!(person, Standing::User(person) if person.user_id == user.id));
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_resolver_alone_stops_resolving_a_revoked_client_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let (row_id, ..) = seed_app_client(&db, " sms:send ").await;
    let key = TestKey::app();
    let registered = register_app_key(&db, &row_id, &key).await;
    assert_eq!(resolved(&db, &key, CoseAlg::Ed25519).await, 1);

    revoke_client_key(&db, registered.id).await;

    assert_eq!(resolved(&db, &key, CoseAlg::Ed25519).await, 0);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_directory_alone_refuses_a_revoked_client_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let directory = SignerDirectory::new(db.clone());
    let (row_id, ..) = seed_app_client(&db, " sms:send ").await;
    let key = TestKey::app();
    let registered = register_app_key(&db, &row_id, &key).await;
    assert!(!standing_is_refused(&directory, &key, CoseAlg::Ed25519).await);

    revoke_client_key(&db, registered.id).await;

    assert!(standing_is_refused(&directory, &key, CoseAlg::Ed25519).await);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_resolver_alone_stops_resolving_a_revoked_console_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let key = TestKey::console();
    let user = seed_user_with_console_key(&db, "admin", &key).await;
    assert_eq!(resolved(&db, &key, CoseAlg::Esp256).await, 1);

    revoke_console_key(&db, &user.id).await;

    assert_eq!(resolved(&db, &key, CoseAlg::Esp256).await, 0);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_directory_alone_refuses_a_revoked_console_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let directory = SignerDirectory::new(db.clone());
    let key = TestKey::console();
    let user = seed_user_with_console_key(&db, "admin", &key).await;
    assert!(!standing_is_refused(&directory, &key, CoseAlg::Esp256).await);

    revoke_console_key(&db, &user.id).await;

    assert!(standing_is_refused(&directory, &key, CoseAlg::Esp256).await);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_resolver_alone_stops_resolving_an_expired_console_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let person = seed_user_with_console_key(&db, "admin", &TestKey::console()).await;
    let key = TestKey::console();
    pair_console_key(&db, &person.id, &key, Duration::seconds(-30)).await;

    assert_eq!(resolved(&db, &key, CoseAlg::Esp256).await, 0);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_directory_alone_refuses_an_expired_console_key() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let directory = SignerDirectory::new(db.clone());
    let person = seed_user_with_console_key(&db, "admin", &TestKey::console()).await;
    let key = TestKey::console();
    pair_console_key(&db, &person.id, &key, Duration::minutes(5)).await;
    // Alive now, expired five minutes later: the directory reads the clock it
    // is handed, so this fails only if it checks the key's expiry.
    assert!(!standing_is_refused(&directory, &key, CoseAlg::Esp256).await);
    let later = Utc::now() + Duration::minutes(10);
    let verdict = directory
        .standing(&signer_of(&key, CoseAlg::Esp256), later)
        .await;
    assert!(
        matches!(verdict, Err(CratestackError::Unauthorized(_))),
        "{verdict:?}"
    );
}

/// A thumbprint names one key of one algorithm. A `ClientKey`'s presented
/// under ESP256, or a `ConsoleKey`'s under Ed25519, is a contradiction the
/// directory refuses rather than resolving by whichever table holds the row.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_client_key_presented_under_esp256_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let directory = SignerDirectory::new(db.clone());
    let (row_id, ..) = seed_app_client(&db, " sms:send ").await;
    let key = TestKey::app();
    register_app_key(&db, &row_id, &key).await;

    assert!(!standing_is_refused(&directory, &key, CoseAlg::Ed25519).await);
    assert!(standing_is_refused(&directory, &key, CoseAlg::Esp256).await);
    assert_eq!(resolved(&db, &key, CoseAlg::Esp256).await, 0);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_console_key_presented_under_ed25519_is_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let directory = SignerDirectory::new(db.clone());
    let key = TestKey::console();
    seed_user_with_console_key(&db, "admin", &key).await;

    assert!(!standing_is_refused(&directory, &key, CoseAlg::Esp256).await);
    assert!(standing_is_refused(&directory, &key, CoseAlg::Ed25519).await);
    assert_eq!(resolved(&db, &key, CoseAlg::Ed25519).await, 0);
}

// ---------------------------------------------------------------------------
// The pairing chain: an admin must not be able to become the owner.
//
// `console:pair` lets an app client attach a console key to any linked person.
// With it, three sealed steps would turn an admin into an owner without ever
// writing a role (so the owner-grant guard never fires):
//
//   1. give an app client `console:pair`            (guard 1)
//   2. register the admin's own key for that client (guard 2)
//   3. sign as it and attach a console key to the owner (guard 3)
//
// Each guard is enough on its own, and the tests below show it by walking the
// chain with the other two steps done by fixtures.
// ---------------------------------------------------------------------------

/// Which steps of the chain a fixture performs instead of the admin's own
/// sealed call, standing for "that guard is not there".
#[derive(Clone, Copy, Default)]
struct Bypass {
    grant: bool,
    key: bool,
    attach: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Chain {
    /// The sealed call of this step was refused, with this status.
    StoppedAt(&'static str, u16),
    /// Every step went through and the owner-only action ran with this status.
    Reached(u16),
}

/// A linked owner (the person vaam would name) and the vaam id that names them.
async fn seed_linked_owner(db: &Cratestack) -> (schema::User, String) {
    let vaam = format!("vaam-owner-{}", suffix());
    let user = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("pending-{}", suffix()),
            email: format!("{vaam}@vaam.invalid"),
            displayName: "The Owner".to_owned(),
            roleKey: "owner".to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the owner");
    db.federated_link()
        .create(schema::CreateFederatedLinkInput {
            issuer: sms_api::console_keys::FEDERATION_ISSUER.to_owned(),
            subject: vaam.clone(),
            userId: user.id.clone(),
        })
        .run(&sys())
        .await
        .expect("linking the owner");
    (user, vaam)
}

async fn set_client_scopes(db: &Cratestack, row_id: &str, scopes: &str) {
    let row = db
        .app_client()
        .find_unique(row_id.to_owned())
        .run(&sys())
        .await
        .expect("reading the client")
        .expect("the client exists");
    db.app_client()
        .update(row.id)
        .set(schema::UpdateAppClientInput {
            scopes: Some(scopes.to_owned()),
            ..Default::default()
        })
        .if_match(row.version)
        .run(&owner())
        .await
        .expect("setting the scopes");
}

fn possession(
    scalar: &[u8; 32],
    public_key: &str,
    issued_at: chrono::DateTime<Utc>,
) -> schema::RegisterConsoleKeyInput {
    use p256::ecdsa::signature::Signer as _;
    let signing = p256::ecdsa::SigningKey::from_slice(scalar).expect("a valid scalar");
    let signature: p256::ecdsa::Signature = signing.sign(
        &sms_api::console_keys::possession_message(public_key, issued_at.timestamp()),
    );
    schema::RegisterConsoleKeyInput {
        vaamUserId: String::new(),
        displayName: None,
        publicKey: public_key.to_owned(),
        possessionProof: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        issuedAt: issued_at,
    }
}

/// An admin with a paired console key walks the chain, step by step, against
/// the real router. Returns where it stopped, or what the owner-only action
/// the finished chain unlocks answered.
async fn walk_the_chain(rig: &Rig, bypass: Bypass) -> Chain {
    let admin_key = TestKey::console();
    seed_user_with_console_key(&rig.db, "admin", &admin_key).await;
    let admin = client(&rig.base, &admin_key);

    // An ordinary client of the admin's choosing, and a key the admin holds.
    let (client_row, ..) = seed_app_client(&rig.db, " sms:send ").await;
    let app_key = TestKey::app();
    let (victim, vaam) = seed_linked_owner(&rig.db).await;

    // 1. Give the client `console:pair`.
    if bypass.grant {
        set_client_scopes(&rig.db, &client_row, " sms:send console:pair ").await;
    } else {
        let row = rig
            .db
            .app_client()
            .find_unique(client_row.clone())
            .run(&sys())
            .await
            .unwrap()
            .unwrap();
        let reply = admin
            .request("PATCH", "/app_clients/{id}")
            .params(&[&client_row])
            .if_match(&format!("\"{}\"", row.version))
            .json(&serde_json::json!({"scopes": " sms:send console:pair "}))
            .send()
            .await;
        if reply.status >= 400 {
            return Chain::StoppedAt("grant the scope", reply.status);
        }
    }

    // 2. Register the admin's own key for it.
    if bypass.key {
        register_app_key(&rig.db, &client_row, &app_key).await;
    } else {
        let reply = admin
            .request("POST", "/$procs/registerAppClientKey")
            .json(&serde_json::json!({"args": {
                "appClientId": client_row,
                "publicKey": app_key.public_key_b64u(),
            }}))
            .send()
            .await;
        if reply.status >= 400 {
            return Chain::StoppedAt("register a key", reply.status);
        }
    }

    // 3. Sign as the client and attach a console key to the owner.
    let mut scalar = [0_u8; 32];
    scalar[..16].copy_from_slice(cratestack::uuid::Uuid::new_v4().as_bytes());
    scalar[0] = 0x01;
    scalar[31] |= 0x01;
    let console_key = TestKey::console_from_scalar(&scalar);
    if bypass.attach {
        pair_console_key(&rig.db, &victim.id, &console_key, Duration::days(30)).await;
    } else {
        let mut input = possession(&scalar, &console_key.public_key_b64u(), Utc::now());
        input.vaamUserId = vaam;
        let reply = client(&rig.base, &app_key)
            .request("POST", "/$procs/registerConsoleKey")
            .json(&serde_json::json!({"args": input}))
            .send()
            .await;
        if reply.status >= 400 {
            return Chain::StoppedAt("attach a key to the owner", reply.status);
        }
    }

    // 4. Act as the owner: an owner-only write (`User` deletion is owner-only).
    let target = seed_user_with_console_key(&rig.db, "support", &TestKey::console()).await;
    let reply = client(&rig.base, &console_key)
        .request("DELETE", "/users/{id}")
        .params(&[&target.id])
        .if_match(&format!("\"{}\"", target.version))
        .send()
        .await;
    Chain::Reached(reply.status)
}

/// The control that keeps the three tests below honest: with every step done
/// by a fixture the chain does reach an owner-only action, so a refusal in the
/// other tests is the guard's and not a fixture that never worked.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_pairing_chain_reaches_owner_standing_when_no_guard_stands_in_the_way() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let outcome = walk_the_chain(
        &rig,
        Bypass {
            grant: true,
            key: true,
            attach: true,
        },
    )
    .await;
    assert!(
        matches!(outcome, Chain::Reached(status) if status < 300),
        "{outcome:?}"
    );
}

/// Guard 1: an admin cannot give a client `console:pair`, by PATCH or by
/// `provisionAppClient`, and the other two steps are not what stops it here.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_admin_cannot_grant_the_pair_scope() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let outcome = walk_the_chain(
        &rig,
        Bypass {
            grant: false,
            key: true,
            attach: true,
        },
    )
    .await;
    assert_eq!(outcome, Chain::StoppedAt("grant the scope", 403));

    // The other way to hand it out, and the owner is no exception over HTTP.
    let admin_key = TestKey::console();
    seed_user_with_console_key(&rig.db, "admin", &admin_key).await;
    let (_, _, app_id) = seed_app_client(&rig.db, " sms:send ").await;
    let label = format!("pairing-{}", suffix());
    let reply = client(&rig.base, &admin_key)
        .request("POST", "/$procs/provisionAppClient")
        .json(&serde_json::json!({"args": {
            "appId": app_id,
            "label": label,
            "scopes": ["sms:send", "console:pair"],
            "publicKey": TestKey::app().public_key_b64u(),
        }}))
        .send()
        .await;
    assert_eq!(reply.status, 403, "{}", reply.text());
    let made = rig
        .db
        .app_client()
        .find_many()
        .where_expr(FilterExpr::from(schema::app_client::label().eq(label)))
        .run(&sys())
        .await
        .unwrap();
    assert!(made.is_empty(), "no client was provisioned");

    let owner_key = TestKey::console();
    seed_user_with_console_key(&rig.db, "owner", &owner_key).await;
    let (row, ..) = seed_app_client(&rig.db, " sms:send ").await;
    let version = rig
        .db
        .app_client()
        .find_unique(row.clone())
        .run(&sys())
        .await
        .unwrap()
        .unwrap()
        .version;
    let reply = client(&rig.base, &owner_key)
        .request("PATCH", "/app_clients/{id}")
        .params(&[&row])
        .if_match(&format!("\"{version}\""))
        .json(&serde_json::json!({"scopes": " console:pair "}))
        .send()
        .await;
    assert_eq!(reply.status, 403, "{}", reply.text());
}

/// Guard 2: a client that holds `console:pair` takes no key from a signed call.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_admin_cannot_register_a_key_for_a_pairing_client() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let outcome = walk_the_chain(
        &rig,
        Bypass {
            grant: true,
            key: false,
            attach: true,
        },
    )
    .await;
    assert_eq!(outcome, Chain::StoppedAt("register a key", 403));
}

/// Guard 3: even a pairing client with the admin's key cannot reach an owner.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_pairing_client_cannot_attach_a_key_to_an_owner() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let outcome = walk_the_chain(
        &rig,
        Bypass {
            grant: true,
            key: true,
            attach: false,
        },
    )
    .await;
    assert_eq!(outcome, Chain::StoppedAt("attach a key to the owner", 403));
}

/// With nothing bypassed the first step stops it, and the owner is untouched.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_whole_pairing_chain_is_stopped_at_its_first_step() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let outcome = walk_the_chain(&rig, Bypass::default()).await;
    assert_eq!(outcome, Chain::StoppedAt("grant the scope", 403));
}
