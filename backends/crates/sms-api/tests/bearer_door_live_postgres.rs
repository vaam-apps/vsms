//! The bearer door (VSA-3b) against a real Postgres and a real HTTP round trip,
//! with a verifier that needs no RSA key: the door's behaviour is what is under
//! test, and the real token checks (`sms-gateway`'s `op_bearer`) are covered by
//! its own unit tests and by `machine_tokens_live_postgres` with real tokens.
//!
//! What it proves, each against the signed door of the same client:
//!
//! - **one policy**: the same Layer 1 and Layer 2 answers through either door, a
//!   token's scope claim only narrows, and `console:pair` never rides a token;
//! - **one principal**: one rate-limit bucket and one idempotency namespace;
//! - **the standing is read from rows**: an inactive `OauthClient` (every
//!   request), an inactive or retired `AppClient`, a client linked to another
//!   `AppClient`, and a client with none are all the layer's own coarse 401;
//! - **a person's token (VSA-4b)**: the same door admits the access token
//!   `human_login` mints, as the `User` it names with the role their row says,
//!   one bucket and one idempotency namespace with that person's console key,
//!   and **never as an `owner` or an `admin`** (a 403, `DEVICE_REQUIRED`), the
//!   two guards proven apart;
//! - **what the doors do to the envelope**: signed beside `Authorization` is a
//!   400, a token caller is never sealed and never earns a contract-lock 426,
//!   `keypair` off refuses a signed body (415) and a key registration (422).
//!
//! ```bash
//! cargo test -p sms-api --test bearer_door_live_postgres -- --ignored --test-threads=1
//! ```

#[path = "support/sealed_fixtures.rs"]
mod sealed_fixtures;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use sealed_fixtures::{
    api_router_doors, client, owner, register_app_key, seed_sender, serve, unique_mtn_msisdn,
};
use sms_api::auth::system_context;
use sms_api::schema::{self, Cratestack, message};
use sms_api::{ApiDoors, BearerRefusal, BearerVerifier, HmacWebhooks, VerifiedBearer};
use sms_test_support::sealed::TestKey;

const PEPPER: &str = "bearer-door-live-test-pepper-well-over-the-minimum-length";

/// What the envelope layer says to anything it will not open, byte for byte: a
/// refusal by the bearer door must be indistinguishable from it.
const COARSE_REFUSAL: &str =
    r#"{"code":"UNAUTHORIZED","message":"request could not be authenticated","details":null}"#;

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn sys() -> CratestackContext {
    system_context("bearer-door-live-test")
}

fn suffix() -> String {
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

/// Answers from a table: a token string is a key into it. The string is the
/// whole credential; what it maps to is what a real verifier would have read out
/// of the claims.
#[derive(Default)]
struct Table(Mutex<HashMap<String, Result<VerifiedBearer, BearerRefusal>>>);

#[async_trait::async_trait]
impl BearerVerifier for Table {
    async fn verify(&self, token: &str) -> Result<VerifiedBearer, BearerRefusal> {
        self.0
            .lock()
            .unwrap()
            .get(token)
            .cloned()
            .unwrap_or(Err(BearerRefusal::Malformed))
    }
}

impl Table {
    fn token(&self, client_id: &str, scope: Option<&str>) -> String {
        let token = format!("tok-{}", suffix());
        self.0.lock().unwrap().insert(
            token.clone(),
            Ok(VerifiedBearer::Machine {
                client_id: client_id.to_owned(),
                scope: scope.map(str::to_owned),
            }),
        );
        token
    }

    /// A person's access token: `user_id` signed in through `console_id`.
    fn person(&self, user_id: &str, console_id: &str) -> String {
        let token = format!("tok-{}", suffix());
        self.0.lock().unwrap().insert(
            token.clone(),
            Ok(VerifiedBearer::Person {
                user_id: user_id.to_owned(),
                client_id: console_id.to_owned(),
            }),
        );
        token
    }

    fn refused(&self, refusal: BearerRefusal) -> String {
        let token = format!("tok-{}", suffix());
        self.0.lock().unwrap().insert(token.clone(), Err(refusal));
        token
    }
}

/// An `App`, an `AppClient` holding `scopes`, and the active `OauthClient` that
/// links to it, the way `provision-client` writes them. Returns the `AppClient`
/// row id, its `clientId`, the `OauthClient` row id and the app id.
struct Seeded {
    app_client_row: String,
    client_id: String,
    oauth_row: String,
    app_id: String,
}

async fn seed(db: &Cratestack, scopes: &str) -> Seeded {
    seed_split(db, scopes, scopes).await
}

/// [`seed`], with the `OauthClient` row listing `oauth_scopes` apart from the
/// `AppClient`'s `app_scopes`: a token's ceiling is the row `/token` checks.
async fn seed_split(db: &Cratestack, app_scopes: &str, oauth_scopes: &str) -> Seeded {
    let scopes = app_scopes;
    let tag = suffix();
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "bearer door live test app".to_owned(),
            slug: format!("bearer-door-{tag}"),
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
    let client_id = format!("appc_bearer_{tag}");
    let app_client = db
        .app_client()
        .create(schema::CreateAppClientInput {
            appId: app.id.clone(),
            clientId: client_id.clone(),
            label: "bearer door live test client".to_owned(),
            scopes: scopes.to_owned(),
            lastUsedAt: None,
            retiredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the app client");
    let oauth = link_oauth_client(db, &client_id, Some(app_client.id.clone()), oauth_scopes).await;
    Seeded {
        app_client_row: app_client.id,
        client_id,
        oauth_row: oauth,
        app_id: app.id,
    }
}

async fn link_oauth_client(
    db: &Cratestack,
    client_id: &str,
    app_client_row: Option<String>,
    scopes: &str,
) -> String {
    db.oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: client_id.to_owned(),
            appClientId: app_client_row,
            tokenEndpointAuthMethod: schema::ClientAuthMethod::private_key_jwt,
            jwks: Some("{\"keys\":[{\"kty\":\"RSA\",\"n\":\"AQAB\",\"e\":\"AQAB\"}]}".to_owned()),
            grantTypes: " client_credentials ".to_owned(),
            scopes: scopes.to_owned(),
            redirectUris: " ".to_owned(),
            requirePkce: false,
        })
        .run(&sys())
        .await
        .expect("seeding the oauth client")
        .id
}

/// The console's `OauthClient` the way `seed-console-client` writes it: public
/// (`tokenEndpointAuthMethod = none`), linked to no `AppClient`, the code grant.
async fn seed_console_with(
    db: &Cratestack,
    method: schema::ClientAuthMethod,
    app_client_row: Option<String>,
) -> (String, String) {
    let client_id = format!("console_{}", suffix());
    let row = db
        .oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: client_id.clone(),
            appClientId: app_client_row,
            tokenEndpointAuthMethod: method,
            // The table's CHECK: a key-authenticated client has a key set, a
            // public one has none.
            jwks: (method == schema::ClientAuthMethod::private_key_jwt)
                .then(|| "{\"keys\":[{\"kty\":\"RSA\",\"n\":\"AQAB\",\"e\":\"AQAB\"}]}".to_owned()),
            grantTypes: " authorization_code ".to_owned(),
            scopes: " openid profile ".to_owned(),
            redirectUris: " https://console.example/cb ".to_owned(),
            requirePkce: true,
        })
        .run(&sys())
        .await
        .expect("seeding the console client")
        .id;
    (client_id, row)
}

async fn seed_console(db: &Cratestack) -> (String, String) {
    seed_console_with(db, schema::ClientAuthMethod::none, None).await
}

/// A fresh active user in `role_key` with no key: a person who has only a
/// password session.
async fn seed_person(db: &Cratestack, role_key: &str) -> schema::User {
    let tag = suffix();
    db.user()
        .create(schema::CreateUserInput {
            subject: format!("pending-{tag}"),
            email: format!("{tag}@fixture.invalid"),
            displayName: "Bearer Door Person".to_owned(),
            roleKey: role_key.to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding the person")
}

async fn set_role(db: &Cratestack, user_id: &str, role_key: &str) {
    let current = db
        .user()
        .find_unique(user_id.to_owned())
        .run(&sys())
        .await
        .unwrap()
        .unwrap();
    db.user()
        .update(user_id.to_owned())
        .set(schema::UpdateUserInput {
            roleKey: Some(role_key.to_owned()),
            ..Default::default()
        })
        .if_match(current.version)
        .run(&owner())
        .await
        .expect("changing the person's role");
}

async fn set_oauth_active(db: &Cratestack, oauth_row: &str, active: bool) {
    db.oauth_client()
        .update(oauth_row.to_owned())
        .set(schema::UpdateOauthClientInput {
            active: Some(active),
            ..Default::default()
        })
        .run(&sys())
        .await
        .expect("flipping OauthClient.active");
}

async fn app_client_version(db: &Cratestack, row: &str) -> i64 {
    db.app_client()
        .find_unique(row.to_owned())
        .run(&sys())
        .await
        .unwrap()
        .unwrap()
        .version
}

async fn retire_app_client(db: &Cratestack, row: &str) {
    let version = app_client_version(db, row).await;
    db.app_client()
        .update(row.to_owned())
        .set(schema::UpdateAppClientInput {
            retiredAt: Some(Some(Utc::now())),
            ..Default::default()
        })
        .if_match(version)
        .run(&owner())
        .await
        .expect("retiring the app client");
}

async fn deactivate_app_client(db: &Cratestack, row: &str) {
    sealed_fixtures::deactivate_app_client(db, row).await;
}

struct Rig {
    db: Cratestack,
    table: Arc<Table>,
    base: String,
    sender: String,
}

async fn rig_with(keypair: bool, bearer: bool) -> Rig {
    let db = db().await;
    let table = Arc::new(Table::default());
    let verifier: Option<Arc<dyn BearerVerifier>> = bearer.then(|| table.clone() as _);
    let doors = ApiDoors::new(keypair, verifier).expect("a door is open");
    let sender = seed_sender(&db).await;
    let base = serve(api_router_doors(
        &db,
        PEPPER,
        HmacWebhooks::Permitted,
        doors,
    ))
    .await;
    Rig {
        db,
        table,
        base,
        sender,
    }
}

/// Both doors open: the deployment `auth.methods: [keypair, machine_tokens]`.
async fn rig() -> Rig {
    rig_with(true, true).await
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

/// A bearer call: `body` (if any) as JSON.
async fn bearer_call(
    rig: &Rig,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<&serde_json::Value>,
) -> (u16, String, Option<String>) {
    let mut request = http()
        .request(method, format!("{}{path}", rig.base))
        .bearer_auth(token)
        .header("accept", "application/json");
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(serde_json::to_vec(body).unwrap());
    }
    let response = request.send().await.expect("sending a bearer call");
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    (
        status,
        response.text().await.expect("reading the answer"),
        content_type,
    )
}

fn send_body(rig: &Rig, client_ref: &str) -> serde_json::Value {
    serde_json::json!({"args": {
        "to": unique_mtn_msisdn(),
        "body": "a send through either door",
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

// --- one policy -------------------------------------------------------------

/// The same client gets the same answer through either door, on a procedure, on a
/// Layer 2 route and on a Layer 1 refusal, and the context is the app's: the
/// message a token sends belongs to the client's app.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_token_and_a_signature_from_the_same_client_get_the_same_answers() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let seeded = seed(&rig.db, " sms:send job:read ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &seeded.app_client_row, &key).await;
    let api = client(&rig.base, &key);
    let token = rig.table.token(&seeded.client_id, None);

    // A procedure with a body: both send, both as the client's app.
    let signed_ref = format!("signed-{}", suffix());
    let signed = api
        .request("POST", "/$procs/sendMessage")
        .json(&send_body(&rig, &signed_ref))
        .send()
        .await;
    assert!(signed.sealed, "{}", signed.text());
    assert_eq!(signed.status, 200, "{}", signed.text());
    let client_ref = format!("bearer-{}", suffix());
    let (status, body, content_type) = bearer_call(
        &rig,
        reqwest::Method::POST,
        "/$procs/sendMessage",
        &token,
        Some(&send_body(&rig, &client_ref)),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        content_type
            .as_deref()
            .unwrap_or_default()
            .starts_with("application/json"),
        "a token caller gets plain JSON, not {content_type:?}"
    );
    assert!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["messageId"].is_string());
    assert_eq!(
        messages_with_client_ref(&rig.db, &seeded.app_id, &client_ref).await,
        1,
        "the token's message belongs to the client's app"
    );

    // A generated read, scoped by policy to the client's own app: the message
    // either door sent is there, and nobody else's.
    let page = api.request("GET", "/messages").send().await;
    let (status, body, _) =
        bearer_call(&rig, reqwest::Method::GET, "/messages", &token, None).await;
    assert_eq!(
        (page.status, status),
        (200, 200),
        "{} / {body}",
        page.text()
    );
    let listed: serde_json::Value = serde_json::from_str(&body).unwrap();
    let items = listed["items"].as_array().expect("a page of messages");
    assert!(
        items.len() == 2
            && items
                .iter()
                .all(|item| item["appId"] == seeded.app_id.as_str()),
        "both sends, and only the client's app's: {body}"
    );
    assert_eq!(page.json()["items"].as_array().map(Vec::len), Some(2));

    // Layer 2: `job:read` is a scope the client holds, so both doors read.
    let jobs = api.request("GET", "/jobs").send().await;
    let (status, ..) = bearer_call(&rig, reqwest::Method::GET, "/jobs", &token, None).await;
    assert_eq!((jobs.status, status), (200, 200));

    // Layer 1: an app principal never writes a provider, through either door.
    let patch = api
        .request("PATCH", "/providers/{id}")
        .params(&["doesnotexist"])
        .json(&serde_json::json!({"displayName": "x"}))
        .send()
        .await;
    let (status, ..) = bearer_call(
        &rig,
        reqwest::Method::PATCH,
        "/providers/doesnotexist",
        &token,
        Some(&serde_json::json!({"displayName": "x"})),
    )
    .await;
    assert_eq!((patch.status, status), (403, 403), "{}", patch.text());
}

/// The token's scope claim narrows what the client holds and never grants: a
/// claim without `job:read` loses it, a claim naming a scope the client does not
/// hold gains nothing.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_scope_claim_narrows_and_never_grants() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let seeded = seed(&rig.db, " sms:send job:read ").await;

    let narrowed = rig.table.token(&seeded.client_id, Some("sms:send"));
    let (status, body, _) = bearer_call(&rig, reqwest::Method::GET, "/jobs", &narrowed, None).await;
    assert_eq!(status, 403, "a claim without job:read loses it: {body}");

    let widened = rig
        .table
        .token(&seeded.client_id, Some("sms:send provider:read route:read"));
    for path in ["/providers", "/routes"] {
        let (status, body, _) = bearer_call(&rig, reqwest::Method::GET, path, &widened, None).await;
        assert_eq!(
            status, 403,
            "{path}: naming a scope the client lacks: {body}"
        );
    }

    let kept = rig.table.token(&seeded.client_id, Some("job:read"));
    let (status, ..) = bearer_call(&rig, reqwest::Method::GET, "/jobs", &kept, None).await;
    assert_eq!(
        status, 200,
        "a claim naming a scope the client holds keeps it"
    );
}

/// A scope on the `AppClient` that the `OauthClient` row never listed is not
/// granted to a token: `/token` only checks a *requested* scope against the row,
/// so a token minted with no `scope` claim must not inherit what an admin added
/// to the `AppClient` alone. The signed door of the same client keeps it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_app_client_scope_the_oauth_client_lacks_is_not_granted_to_a_token() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let seeded = seed_split(&rig.db, " sms:send job:read ", " sms:send ").await;

    // No claim, a claim naming it, and a claim naming only what the row lists.
    for claim in [None, Some("sms:send job:read"), Some("job:read")] {
        let token = rig.table.token(&seeded.client_id, claim);
        let (status, body, _) =
            bearer_call(&rig, reqwest::Method::GET, "/jobs", &token, None).await;
        assert_eq!(
            status, 403,
            "claim {claim:?}: job:read is on the AppClient only: {body}"
        );
    }

    // The row's own scope still works through the same door.
    let token = rig.table.token(&seeded.client_id, None);
    let (status, body, _) = bearer_call(
        &rig,
        reqwest::Method::POST,
        "/$procs/sendMessage",
        &token,
        Some(&send_body(&rig, &format!("ceiling-{}", suffix()))),
    )
    .await;
    assert_eq!(status, 200, "sms:send is on both rows: {body}");

    // The signed door of the same client is not capped by the OAuth row.
    let key = TestKey::app();
    register_app_key(&rig.db, &seeded.app_client_row, &key).await;
    let signed = client(&rig.base, &key).request("GET", "/jobs").send().await;
    assert_eq!(
        signed.status,
        200,
        "the AppClient holds job:read: {}",
        signed.text()
    );
}

/// `console:pair` is dropped from a token whatever the client holds and the
/// token claims: pairing a console is a signed act. The same client, signing,
/// passes the scope check.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn console_pair_never_rides_a_token() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let seeded = seed(&rig.db, " sms:send console:pair ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &seeded.app_client_row, &key).await;
    // Well-formed, so the scope check (the first thing the body does) is what
    // answers; a key this bogus is refused later with a 422 for the signed door.
    let args = serde_json::json!({"args": {
        "vaamUserId": "vaam-user-1",
        "publicKey": "AAAA",
        "possessionProof": "AAAA",
        "issuedAt": Utc::now().to_rfc3339(),
    }});

    let signed = client(&rig.base, &key)
        .request("POST", "/$procs/registerConsoleKey")
        .json(&args)
        .send()
        .await;
    assert_ne!(
        signed.status,
        403,
        "the signed door keeps console:pair, so the scope check passes: {}",
        signed.text()
    );

    for claim in [None, Some("sms:send console:pair"), Some("console:pair")] {
        let token = rig.table.token(&seeded.client_id, claim);
        let (status, body, _) = bearer_call(
            &rig,
            reqwest::Method::POST,
            "/$procs/registerConsoleKey",
            &token,
            Some(&args),
        )
        .await;
        assert_eq!(status, 403, "claim {claim:?}: {body}");
        assert!(body.contains("console:pair"), "claim {claim:?}: {body}");
    }
}

// --- one principal ----------------------------------------------------------

/// One `Idempotency-Key`, two doors: a send sealed once and replayed with a token
/// is the stored answer, not a second message.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn one_idempotency_key_is_one_send_across_both_doors() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let seeded = seed(&rig.db, " sms:send ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &seeded.app_client_row, &key).await;
    let token = rig.table.token(&seeded.client_id, None);
    let client_ref = format!("idem-{}", suffix());
    let body = send_body(&rig, &client_ref);
    let idempotency_key = format!("idem-key-{}", suffix());

    let first = client(&rig.base, &key)
        .request("POST", "/$procs/sendMessage")
        .idempotency_key(&idempotency_key)
        .json(&body)
        .send()
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    let message_id = first.json()["messageId"].clone();

    let replay = http()
        .post(format!("{}/$procs/sendMessage", rig.base))
        .bearer_auth(&token)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("idempotency-key", &idempotency_key)
        .body(serde_json::to_vec(&body).unwrap())
        .send()
        .await
        .expect("replaying with a token");
    assert_eq!(replay.status().as_u16(), 200);
    let replayed: serde_json::Value = replay.json().await.expect("JSON");
    assert_eq!(replayed["messageId"], message_id, "the stored answer");
    assert_eq!(
        messages_with_client_ref(&rig.db, &seeded.app_id, &client_ref).await,
        1,
        "one key, one message"
    );
}

/// Both doors draw from one bucket per client: three calls through any mix spend
/// a burst of three, and the fourth is a 429 whichever door makes it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn both_doors_spend_one_rate_limit_bucket() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let table = Arc::new(Table::default());
    let doors = ApiDoors::new(true, Some(table.clone() as Arc<dyn BearerVerifier>)).unwrap();
    let router = || {
        sms_api::router(
            db.clone(),
            sealed_fixtures::server_keys().into_signer(),
            sms_api::HashPepper::new(PEPPER).unwrap(),
            sms_api::DEFAULT_IDEMPOTENCY_TTL,
            cratestack::ratelimit::RateLimitConfig::new(3, 0.001),
            sms_api::default_source_rate_limit_config(),
            sms_api::ApiAuth {
                hmac_webhooks: HmacWebhooks::Permitted,
                doors: doors.clone(),
            },
        )
        .expect("the envelope layer accepts its configuration")
    };
    let preview = serde_json::json!({"args": {"body": "hello"}});

    for bearer_first in [true, false] {
        let base = serve(router()).await;
        let rig = Rig {
            db: db.clone(),
            table: table.clone(),
            base,
            sender: String::new(),
        };
        let seeded = seed(&db, " sms:send ").await;
        let key = TestKey::app();
        register_app_key(&db, &seeded.app_client_row, &key).await;
        let api = client(&rig.base, &key);
        let token = table.token(&seeded.client_id, None);

        let mut statuses = Vec::new();
        for turn in 0..4 {
            let bearer = (turn % 2 == 0) == bearer_first;
            let status = if bearer {
                bearer_call(
                    &rig,
                    reqwest::Method::POST,
                    "/$procs/previewMessage",
                    &token,
                    Some(&preview),
                )
                .await
                .0
            } else {
                api.request("POST", "/$procs/previewMessage")
                    .json(&preview)
                    .send()
                    .await
                    .status
            };
            statuses.push(status);
        }
        assert_eq!(
            statuses,
            [200, 200, 200, 429],
            "bearer first: {bearer_first}: three calls spend the burst, the fourth is throttled"
        );
    }
}

// --- the standing is read from rows -----------------------------------------

async fn assert_coarse_401(rig: &Rig, token: &str, why: &str) {
    let (status, body, _) = bearer_call(
        rig,
        reqwest::Method::POST,
        "/$procs/previewMessage",
        token,
        Some(&serde_json::json!({"args": {"body": "hello"}})),
    )
    .await;
    assert_eq!(status, 401, "{why}: {body}");
    assert_eq!(body, COARSE_REFUSAL, "{why}");
}

async fn assert_ok(rig: &Rig, token: &str, why: &str) {
    let (status, body, _) = bearer_call(
        rig,
        reqwest::Method::POST,
        "/$procs/previewMessage",
        token,
        Some(&serde_json::json!({"args": {"body": "hello"}})),
    )
    .await;
    assert_eq!(status, 200, "{why}: {body}");
}

/// `OauthClient.active` is read on every request, never cached: a client
/// switched off stops acting on the next call, on a process that already served
/// it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_inactive_oauth_client_stops_acting_on_the_next_request() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let seeded = seed(&rig.db, " sms:send ").await;
    let token = rig.table.token(&seeded.client_id, None);

    assert_ok(&rig, &token, "active").await;
    set_oauth_active(&rig.db, &seeded.oauth_row, false).await;
    assert_coarse_401(&rig, &token, "OauthClient.active = false").await;
    set_oauth_active(&rig.db, &seeded.oauth_row, true).await;
    assert_ok(&rig, &token, "active again").await;
}

/// A client whose `AppClient` is retired or inactive, or that has none, or is
/// linked to somebody else's, has no standing, however good the token.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_client_without_a_live_app_client_has_no_standing() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;

    // The `AppClient` standing is cached for a minute, so each case is a client
    // this process has not served yet.
    let retired = seed(&rig.db, " sms:send ").await;
    retire_app_client(&rig.db, &retired.app_client_row).await;
    assert_coarse_401(
        &rig,
        &rig.table.token(&retired.client_id, None),
        "AppClient.retiredAt",
    )
    .await;

    let inactive = seed(&rig.db, " sms:send ").await;
    deactivate_app_client(&rig.db, &inactive.app_client_row).await;
    assert_coarse_401(
        &rig,
        &rig.table.token(&inactive.client_id, None),
        "AppClient.active = false",
    )
    .await;

    let unlinked_id = format!("appc_unlinked_{}", suffix());
    link_oauth_client(&rig.db, &unlinked_id, None, " sms:send ").await;
    assert_coarse_401(
        &rig,
        &rig.table.token(&unlinked_id, None),
        "OauthClient.appClientId is null",
    )
    .await;

    // An OauthClient pointing at another client's AppClient: the token names
    // `appc_crossed_*`, the row it resolves to belongs to `victim`.
    let victim = seed(&rig.db, " sms:send job:read ").await;
    let crossed_id = format!("appc_crossed_{}", suffix());
    link_oauth_client(
        &rig.db,
        &crossed_id,
        Some(victim.app_client_row.clone()),
        " sms:send ",
    )
    .await;
    assert_coarse_401(
        &rig,
        &rig.table.token(&crossed_id, None),
        "the AppClient's clientId is not the token's sub",
    )
    .await;

    assert_coarse_401(
        &rig,
        &rig.table.token(&format!("appc_nobody_{}", suffix()), None),
        "no OauthClient at all",
    )
    .await;
}

/// Every refusal the verifier can make is the same bytes as a forged signature's.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn every_verifier_refusal_is_the_layers_own_coarse_401() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    for refusal in [
        BearerRefusal::Malformed,
        BearerRefusal::MissingKid,
        BearerRefusal::UnknownKid,
        BearerRefusal::Signature,
        BearerRefusal::Issuer,
        BearerRefusal::Audience,
        BearerRefusal::Expired,
        BearerRefusal::NotYetValid,
        BearerRefusal::HumanToken,
        BearerRefusal::MachineToken,
        BearerRefusal::IdToken,
        BearerRefusal::SenderConstrained,
    ] {
        assert_coarse_401(&rig, &rig.table.refused(refusal), &format!("{refusal:?}")).await;
    }
}

// --- the doors and the envelope ---------------------------------------------

/// One caller, one credential: a signed request carrying `Authorization` is a 400
/// whichever door is open, and nothing runs.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_signed_request_beside_an_authorization_header_is_a_400() {
    let _guard = TEST_MUTEX.lock().await;
    for (keypair, bearer) in [(true, true), (true, false)] {
        let rig = rig_with(keypair, bearer).await;
        let seeded = seed(&rig.db, " sms:send ").await;
        let key = TestKey::app();
        register_app_key(&rig.db, &seeded.app_client_row, &key).await;
        let token = rig.table.token(&seeded.client_id, None);
        let client_ref = format!("both-{}", suffix());

        let reply = client(&rig.base, &key)
            .request("POST", "/$procs/sendMessage")
            .header("authorization", &format!("Bearer {token}"))
            .json(&send_body(&rig, &client_ref))
            .send()
            .await;
        assert_eq!(
            reply.status,
            400,
            "keypair {keypair}, bearer {bearer}: {}",
            reply.text()
        );
        assert!(
            reply
                .text()
                .contains("cannot also carry an Authorization header"),
            "{}",
            reply.text()
        );
        assert_eq!(
            messages_with_client_ref(&rig.db, &seeded.app_id, &client_ref).await,
            0
        );
    }
}

/// A token caller is never sealed: asking for `application/cose` with a valid
/// nonce, the way a sealed client would, still earns plain JSON (not a sealed
/// answer, and not the contract-lock 426 a sealed exchange can earn), in both
/// configurations that open the door.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_token_caller_is_never_sealed_and_never_earns_a_contract_426() {
    let _guard = TEST_MUTEX.lock().await;
    for (keypair, bearer) in [(true, true), (false, true)] {
        let rig = rig_with(keypair, bearer).await;
        let seeded = seed(&rig.db, " sms:send ").await;
        let token = rig.table.token(&seeded.client_id, None);

        let response = http()
            .post(format!("{}/$procs/previewMessage", rig.base))
            .bearer_auth(&token)
            .header("accept", "application/cose, application/json")
            .header("cratestack-nonce", "AAAAAAAAAAAAAAAAAAAAAA")
            .header("content-type", "application/json")
            .body(r#"{"args":{"body":"hello"}}"#)
            .send()
            .await
            .expect("sending");
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = response.text().await.unwrap_or_default();
        assert_eq!(status, 200, "keypair {keypair}: {body}");
        assert!(
            content_type.starts_with("application/json"),
            "keypair {keypair}: not sealed, got {content_type:?}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&body).is_ok(),
            "keypair {keypair}: plain JSON, got {body}"
        );
    }
}

/// With both doors open, a method the schema does not generate on a generated
/// path is the layer's own 405, not the door's 401: the layer's `unresolved`
/// handling fails closed whatever the doors are.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_method_the_schema_does_not_generate_is_a_405_with_both_doors_open() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let response = http()
        .put(format!("{}/messages", rig.base))
        .send()
        .await
        .expect("sending");
    assert_eq!(response.status().as_u16(), 405);
}

/// The signed door alone: a token is a 401, the layer's own, and nothing runs.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_keypair_alone_a_token_is_the_layers_401() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig_with(true, false).await;
    let seeded = seed(&rig.db, " sms:send ").await;
    let token = rig.table.token(&seeded.client_id, None);
    assert_coarse_401(&rig, &token, "keypair only").await;
}

/// Tokens alone (`keypair` off): a token works, a signed body is refused by the
/// layer with a 415 and never opened, and registering a key is a 422.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn with_keypair_off_a_token_works_a_signature_is_a_415_and_a_key_registration_a_422() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig_with(false, true).await;
    let seeded = seed(&rig.db, " sms:send console:pair ").await;
    let key = TestKey::app();
    register_app_key(&rig.db, &seeded.app_client_row, &key).await;
    let token = rig.table.token(&seeded.client_id, None);

    assert_ok(&rig, &token, "tokens alone").await;

    let signed = client(&rig.base, &key)
        .request("POST", "/$procs/previewMessage")
        .json(&serde_json::json!({"args": {"body": "hello"}}))
        .send()
        .await;
    assert_eq!(signed.status, 415, "{}", signed.text());
    assert!(
        !signed.sealed,
        "a refusal the layer makes itself is unsealed"
    );

    for (procedure, mention) in [
        ("registerAppClientKey", "provision-client"),
        ("registerConsoleKey", "VSA-5"),
    ] {
        let (status, body, _) = bearer_call(
            &rig,
            reqwest::Method::POST,
            &format!("/$procs/{procedure}"),
            &token,
            Some(&serde_json::json!({"args": {}})),
        )
        .await;
        assert_eq!(status, 422, "{procedure}: {body}");
        assert!(body.contains("auth_method_disabled"), "{procedure}: {body}");
        assert!(body.contains(mention), "{procedure}: {body}");
    }
}

// --- a person's token (VSA-4b) ----------------------------------------------

const PREVIEW: &str = "/$procs/previewMessage";

async fn preview(rig: &Rig, token: &str) -> (u16, String) {
    let (status, body, _) = bearer_call(
        rig,
        reqwest::Method::POST,
        PREVIEW,
        token,
        Some(&serde_json::json!({"args": {"body": "hello"}})),
    )
    .await;
    (status, body)
}

async fn audit_log(rig: &Rig, token: &str) -> (u16, String) {
    let (status, body, _) = bearer_call(
        rig,
        reqwest::Method::POST,
        "/$procs/auditLog",
        token,
        Some(&serde_json::json!({"args": {}})),
    )
    .await;
    (status, body)
}

fn device_required_body() -> String {
    format!(
        r#"{{"code":"FORBIDDEN","message":"{}","details":null}}"#,
        sms_api::DEVICE_REQUIRED
    )
}

/// An `operator` signed in with a password acts with exactly what the role
/// admits: a procedure open to any caller (200), a generated read the role may
/// make (200), and `auditLog`, which the role is not in (403). The same person
/// through a paired console key gets the same three answers.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_persons_token_acts_with_their_role_as_their_console_key_does() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (console, _) = seed_console(&rig.db).await;
    let key = TestKey::console();
    let person = sealed_fixtures::seed_user_with_console_key(&rig.db, "operator", &key).await;
    let token = rig.table.person(&person.id, &console);

    let (status, body) = preview(&rig, &token).await;
    assert_eq!(status, 200, "{body}");
    let (status, body, content_type) =
        bearer_call(&rig, reqwest::Method::GET, "/messages", &token, None).await;
    assert_eq!(status, 200, "{body}");
    assert!(
        content_type
            .as_deref()
            .unwrap_or_default()
            .starts_with("application/json"),
        "a person's token gets plain JSON, not {content_type:?}"
    );
    let (status, body) = audit_log(&rig, &token).await;
    assert_eq!(status, 403, "operator is not an auditor: {body}");
    assert!(
        !body.contains("device") && !body.contains("paired console key"),
        "an operator's 403 is the policy's, not the device cap's: {body}"
    );

    let api = client(&rig.base, &key);
    let signed_preview = api
        .request("POST", PREVIEW)
        .json(&serde_json::json!({"args": {"body": "hello"}}))
        .send()
        .await;
    let signed_messages = api.request("GET", "/messages").send().await;
    let signed_audit = api
        .request("POST", "/$procs/auditLog")
        .json(&serde_json::json!({"args": {}}))
        .send()
        .await;
    assert_eq!(
        (
            signed_preview.status,
            signed_messages.status,
            signed_audit.status
        ),
        (200, 200, 403),
        "the console key answers the same: {}",
        signed_audit.text()
    );

    // The roles that are not device-only keep their own answers: an auditor
    // reads the audit log by token.
    let auditor = seed_person(&rig.db, "auditor").await;
    let token = rig.table.person(&auditor.id, &console);
    let (status, body) = audit_log(&rig, &token).await;
    assert_eq!(
        status, 200,
        "an auditor's token reads the audit log: {body}"
    );
}

/// One person, one bucket and one idempotency namespace, across a token and the
/// person's own console key: three calls in any mix spend a burst of three.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_persons_token_and_console_key_spend_one_bucket_and_one_idempotency_namespace() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let table = Arc::new(Table::default());
    let doors = ApiDoors::new(true, Some(table.clone() as Arc<dyn BearerVerifier>)).unwrap();
    let router = || {
        sms_api::router(
            db.clone(),
            sealed_fixtures::server_keys().into_signer(),
            sms_api::HashPepper::new(PEPPER).unwrap(),
            sms_api::DEFAULT_IDEMPOTENCY_TTL,
            cratestack::ratelimit::RateLimitConfig::new(3, 0.001),
            sms_api::default_source_rate_limit_config(),
            sms_api::ApiAuth {
                hmac_webhooks: HmacWebhooks::Permitted,
                doors: doors.clone(),
            },
        )
        .expect("the envelope layer accepts its configuration")
    };
    let (console, _) = seed_console(&db).await;
    let preview_body = serde_json::json!({"args": {"body": "hello"}});

    for bearer_first in [true, false] {
        let rig = Rig {
            db: db.clone(),
            table: table.clone(),
            base: serve(router()).await,
            sender: String::new(),
        };
        let key = TestKey::console();
        let person = sealed_fixtures::seed_user_with_console_key(&db, "operator", &key).await;
        let api = client(&rig.base, &key);
        let token = table.person(&person.id, &console);
        let mut statuses = Vec::new();
        for turn in 0..4 {
            let bearer = (turn % 2 == 0) == bearer_first;
            statuses.push(if bearer {
                preview(&rig, &token).await.0
            } else {
                api.request("POST", PREVIEW)
                    .json(&preview_body)
                    .send()
                    .await
                    .status
            });
        }
        assert_eq!(
            statuses,
            [200, 200, 200, 429],
            "bearer first: {bearer_first}: a person's two doors are one bucket"
        );
    }

    // One Idempotency-Key, two doors. `sendMessage` serves machines only, so the
    // probe is a key *reused for a different body*: the layer stores the first
    // request under (principal, key), and a second request under the same
    // principal with another body is refused as a reuse of the key. Were the
    // two doors different principals the second would be a fresh 200.
    let rig = Rig {
        db: db.clone(),
        table: table.clone(),
        base: serve(api_router_doors(
            &db,
            PEPPER,
            HmacWebhooks::Permitted,
            doors.clone(),
        ))
        .await,
        sender: String::new(),
    };
    let key = TestKey::console();
    let person = sealed_fixtures::seed_user_with_console_key(&db, "operator", &key).await;
    let token = table.person(&person.id, &console);
    let idempotency_key = format!("person-idem-key-{}", suffix());
    let first = client(&rig.base, &key)
        .request("POST", PREVIEW)
        .idempotency_key(&idempotency_key)
        .json(&serde_json::json!({"args": {"body": "first body"}}))
        .send()
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    let other_body = |body: &str, idempotency_key: &str| {
        let request = http()
            .post(format!("{}{PREVIEW}", rig.base))
            .bearer_auth(&token)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header("idempotency-key", idempotency_key)
            .body(serde_json::to_vec(&serde_json::json!({"args": {"body": body}})).unwrap());
        async move {
            let response = request.send().await.expect("sending");
            let status = response.status().as_u16();
            (status, response.text().await.unwrap_or_default())
        }
    };
    // The layer's exact answer for a key reused with another body: a 422 with the
    // framework's `VALIDATION_ERROR` envelope and `idempotency_key_conflict`. Any
    // other refusal (a 401 from a broken standing, a 429, a 5xx) is not it.
    let (status, text) = other_body("a different body", &idempotency_key).await;
    assert_eq!(
        status, 422,
        "the person's token shares the key's namespace: {text}"
    );
    assert!(text.contains(r#""code":"VALIDATION_ERROR""#), "{text}");
    assert!(text.contains("idempotency_key_conflict"), "{text}");
    let (fresh, text) = other_body("a different body", &format!("fresh-{}", suffix())).await;
    assert_eq!(
        fresh, 200,
        "control: another key is a fresh request: {text}"
    );
}

/// An `owner` or an `admin` behind a person's token is a 403 with the device
/// text and nothing else, on every kind of route; the same account through a
/// paired console key acts with the role.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn an_admin_or_owner_behind_a_persons_token_is_refused_and_behind_a_console_key_is_not() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (console, _) = seed_console(&rig.db).await;

    for role in ["admin", "owner"] {
        let key = TestKey::console();
        let person = sealed_fixtures::seed_user_with_console_key(&rig.db, role, &key).await;
        let token = rig.table.person(&person.id, &console);

        // The same account, through a key a device holds, acts with the role. It
        // goes **first**, on the router the token will use: the key warms the
        // shared standing cache with the admin, and a token that read that
        // cached value without the cap would be let in as one.
        let api = client(&rig.base, &key);
        let users = api.request("GET", "/users").send().await;
        assert_eq!(users.status, 200, "{role} by key: {}", users.text());

        for (method, path) in [
            (reqwest::Method::GET, "/users".to_owned()),
            (reqwest::Method::GET, "/messages".to_owned()),
            (reqwest::Method::POST, PREVIEW.to_owned()),
            (reqwest::Method::POST, "/$procs/auditLog".to_owned()),
            // The promotion an admin could make to themselves: the door refuses
            // it before the owner-grant guard or the policy is asked.
            (reqwest::Method::PATCH, format!("/users/{}", person.id)),
        ] {
            let body = serde_json::json!({"args": {}, "roleKey": "owner"});
            let (status, text, _) =
                bearer_call(&rig, method.clone(), &path, &token, Some(&body)).await;
            assert_eq!(status, 403, "{role} {method} {path}: {text}");
            assert_eq!(text, device_required_body(), "{role} {method} {path}");
        }

        // And the key is unaffected by the refusals.
        let users = api.request("GET", "/users").send().await;
        assert_eq!(users.status, 200, "{role} by key, after: {}", users.text());
    }
}

/// The two guards apart. The directory's own (`person_standing`) refuses an
/// admin with no door and no provider in the way; and the provider's
/// (`GatewayAuth::authenticate`) is proven in `sms_api::auth`'s unit test by
/// handing it a standing the door never saw.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn the_directory_refuses_a_device_only_role_on_its_own() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let (console, _) = seed_console(&db).await;
    let directory = sms_api::signers::SignerDirectory::new(db.clone());
    for role in ["admin", "owner"] {
        let person = seed_person(&db, role).await;
        let refused = directory
            .bearer_standing(&VerifiedBearer::Person {
                user_id: person.id.clone(),
                client_id: console.clone(),
            })
            .await
            .expect_err("a device-only role has no standing behind a token");
        assert!(
            matches!(&refused, cratestack::CratestackError::Forbidden(text) if text == sms_api::DEVICE_REQUIRED),
            "{role}: {refused:?}"
        );
    }
    let operator = seed_person(&db, "operator").await;
    let standing = directory
        .bearer_standing(&VerifiedBearer::Person {
            user_id: operator.id.clone(),
            client_id: console.clone(),
        })
        .await
        .expect("an operator stands");
    assert_eq!(standing.principal(), format!("user:{}", operator.id));
}

/// The audience is a real console, read from the row on every request: an
/// inactive client, one linked to an `AppClient`, one that authenticates with a
/// key (`private_key_jwt`), none at all, and a machine client's id where the
/// console's belongs, are each the coarse 401. A restored console works again
/// at once.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_persons_token_needs_a_live_public_unlinked_console() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let person = seed_person(&rig.db, "operator").await;

    let (console, console_row) = seed_console(&rig.db).await;
    let token = rig.table.person(&person.id, &console);
    assert_eq!(preview(&rig, &token).await.0, 200, "a real console");
    set_oauth_active(&rig.db, &console_row, false).await;
    let (status, body) = preview(&rig, &token).await;
    assert_eq!((status, body.as_str()), (401, COARSE_REFUSAL), "inactive");
    set_oauth_active(&rig.db, &console_row, true).await;
    assert_eq!(preview(&rig, &token).await.0, 200, "active again");

    let seeded = seed(&rig.db, " sms:send ").await;
    let (linked, _) = seed_console_with(
        &rig.db,
        schema::ClientAuthMethod::none,
        Some(seeded.app_client_row.clone()),
    )
    .await;
    let (keyed, _) =
        seed_console_with(&rig.db, schema::ClientAuthMethod::private_key_jwt, None).await;
    for (why, client_id) in [
        ("linked to an AppClient", linked),
        ("not a public client", keyed),
        ("a machine client's id", seeded.client_id.clone()),
        ("no such client", format!("console_nobody_{}", suffix())),
    ] {
        let (status, body) = preview(&rig, &rig.table.person(&person.id, &client_id)).await;
        assert_eq!((status, body.as_str()), (401, COARSE_REFUSAL), "{why}");
    }
}

/// The person is read from rows: an unknown or inactive user is the coarse 401.
/// A deactivation takes effect within the standing cache's minute (a warm router
/// still serves the person, a fresh one refuses), the window a console key has
/// too; a promotion to a device-only role does the same, as a 403.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_persons_standing_is_read_from_rows_within_one_cache_window() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig().await;
    let (console, _) = seed_console(&rig.db).await;

    let (status, body) = preview(&rig, &rig.table.person("usr_nobody", &console)).await;
    assert_eq!((status, body.as_str()), (401, COARSE_REFUSAL), "unknown");
    let gone = seed_person(&rig.db, "operator").await;
    sealed_fixtures::deactivate_user(&rig.db, &gone.id).await;
    let (status, body) = preview(&rig, &rig.table.person(&gone.id, &console)).await;
    assert_eq!((status, body.as_str()), (401, COARSE_REFUSAL), "inactive");

    // Deactivated after the router served them: the cached standing outlives the
    // change by at most the TTL, and a router that has not seen them refuses.
    let doors = || ApiDoors::new(true, Some(rig.table.clone() as Arc<dyn BearerVerifier>)).unwrap();
    let warm = Rig {
        db: rig.db.clone(),
        table: rig.table.clone(),
        base: serve(api_router_doors(
            &rig.db,
            PEPPER,
            HmacWebhooks::Permitted,
            doors(),
        ))
        .await,
        sender: String::new(),
    };
    let leaving = seed_person(&rig.db, "operator").await;
    let token = rig.table.person(&leaving.id, &console);
    assert_eq!(preview(&warm, &token).await.0, 200);
    sealed_fixtures::deactivate_user(&rig.db, &leaving.id).await;
    assert_eq!(
        preview(&warm, &token).await.0,
        200,
        "inside the cache window the warm router still serves them"
    );
    let fresh = Rig {
        db: rig.db.clone(),
        table: rig.table.clone(),
        base: serve(api_router_doors(
            &rig.db,
            PEPPER,
            HmacWebhooks::Permitted,
            doors(),
        ))
        .await,
        sender: String::new(),
    };
    let (status, body) = preview(&fresh, &token).await;
    assert_eq!(
        (status, body.as_str()),
        (401, COARSE_REFUSAL),
        "fresh router"
    );

    // Promoted to admin: the same shape, a 403 and not a 401.
    let rising = seed_person(&rig.db, "operator").await;
    let token = rig.table.person(&rising.id, &console);
    assert_eq!(preview(&warm, &token).await.0, 200);
    set_role(&rig.db, &rising.id, "admin").await;
    let fresh = Rig {
        db: rig.db.clone(),
        table: rig.table.clone(),
        base: serve(api_router_doors(
            &rig.db,
            PEPPER,
            HmacWebhooks::Permitted,
            doors(),
        ))
        .await,
        sender: String::new(),
    };
    let (status, body) = preview(&fresh, &token).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body, device_required_body());
}

/// A person's token with `keypair` off (tokens alone): the same standing, and a
/// signed body is still the layer's 415.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres - see module docs"]
async fn a_persons_token_works_with_keypair_off_and_an_admin_is_still_refused() {
    let _guard = TEST_MUTEX.lock().await;
    let rig = rig_with(false, true).await;
    let (console, _) = seed_console(&rig.db).await;
    let operator = seed_person(&rig.db, "operator").await;
    assert_eq!(
        preview(&rig, &rig.table.person(&operator.id, &console))
            .await
            .0,
        200
    );
    let admin = seed_person(&rig.db, "admin").await;
    let (status, body) = preview(&rig, &rig.table.person(&admin.id, &console)).await;
    assert_eq!((status, body), (403, device_required_body()));
}
