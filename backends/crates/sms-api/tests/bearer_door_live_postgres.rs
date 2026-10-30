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
            Ok(VerifiedBearer {
                client_id: client_id.to_owned(),
                scope: scope.map(str::to_owned),
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
    let oauth = link_oauth_client(db, &client_id, Some(app_client.id.clone())).await;
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
) -> String {
    db.oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: client_id.to_owned(),
            appClientId: app_client_row,
            tokenEndpointAuthMethod: schema::ClientAuthMethod::private_key_jwt,
            jwks: Some("{\"keys\":[{\"kty\":\"RSA\",\"n\":\"AQAB\",\"e\":\"AQAB\"}]}".to_owned()),
            grantTypes: " client_credentials ".to_owned(),
            scopes: " sms:send ".to_owned(),
            redirectUris: " ".to_owned(),
            requirePkce: false,
        })
        .run(&sys())
        .await
        .expect("seeding the oauth client")
        .id
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
    link_oauth_client(&rig.db, &unlinked_id, None).await;
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
    link_oauth_client(&rig.db, &crossed_id, Some(victim.app_client_row.clone())).await;
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
