//! The live gate for #24: Layer 2 (permission/scope enforcement, §5.1 of
//! the design doc) checked over a *real* HTTP round trip — a real signing
//! key, a real `client_credentials` token minted by the OP with a real
//! `scope` claim, `sms_api::router`'s own `require_permission`/
//! `enforce_route_permission` deciding on it, against a real Postgres. Not
//! a hand-built `CoolContext`: see `crates/sms-api/tests/errors_live_postgres.rs`'s
//! own history (#87) for why this repo doesn't trust a green `cargo test`
//! that never went through the real claim-extraction path, and #29's own
//! `claim_live_postgres.rs` for the same lesson landing a second time
//! (silently-zero-rows policy gaps that only a real query ever surfaces).
//!
//! Two anchors:
//!
//! - **`sendMessage`** (`sms:send`), gated by `Procedures::send`'s own
//!   `require_permission` call — the only one of the seven procedures
//!   that's live, and the only route in this deployment a real
//!   `client_credentials` token can ever reach far enough to prove a
//!   genuine *success* case for, not just a denial. Proves (a) an omitted
//!   `scope` is denied, (b) a `scope` that doesn't contain `sms:send` is
//!   denied, and (c) a `scope` that does succeeds end to end (a real
//!   `Message` row lands in Postgres).
//!
//! - **`PATCH /providers/{id}`** (`provider:update`), `sms-api`'s concrete
//!   anchor for #25 (`router::PROVIDER_WRITE_ROUTES`). Proves the same
//!   primitive denies there too — an omitted scope, and separately a
//!   *working* `sms:send`-scoped token that simply isn't the permission
//!   this route wants. What this file deliberately does **not** claim: a
//!   live *success* case on this route. `Provider.update`'s own
//!   `@@allow` (`schema.cstack`) is `hasRole('owner') || hasRole('admin')
//!   || hasRole('operator')` — no `hasRole('app')` — and `GatewayAuth`
//!   (this deployment's only `AuthProvider`) never mints any role but
//!   `"app"` or `"system"` for a real token, because no human-login path
//!   exists yet (`sms_auth::op`'s own module doc; #23/#24/#25's tracked
//!   scope cut). Layer 1 alone already refuses every token this
//!   deployment can currently issue on this route, so there is no token
//!   this suite could request from the real `/token` endpoint that would
//!   reach a Layer-2 *allow* here — proving one would need a role-bearing
//!   (human) token issuance path, which is future work, not #24's. See
//!   `router::PROVIDER_WRITE_ROUTES`'s own doc for the same point made
//!   from the production-code side.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-auth --test rbac_layer2_live_postgres -- --ignored
//! ```

use std::sync::Arc;

use authkestra_axum::helpers::AxumError;
use authkestra_axum::op::axum_token_handler;
use authkestra_engine::token::jwk::Jwk;
use authkestra_engine::TokenManager;
use authkestra_op::config::OpConfig;
use authkestra_op::handlers::discovery::OidcDiscovery;
use authkestra_op::handlers::jwks::JwksResponse;
use authkestra_op::OpStore;
use axum::extract::{FromRef, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::CoolContext;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};
use sms_api::GatewayAuth;

fn sys() -> CoolContext {
    Principal {
        sub: "rbac-layer2-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "rbac-layer2-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

/// A fresh MSISDN under the seeded `67x` (mtn) prefix — see
/// `crates/sms-api/tests/send_message_live_postgres.rs`'s own copy of this
/// helper for why it needs cross-run, not just cross-call, uniqueness.
fn unique_mtn_msisdn() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .subsec_nanos();
    let unique = (u64::from(nanos) + n) % 1_000_000;
    format!("+237677{unique:06}")
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

// --- The OP's routes, duplicated from `app/sms-gateway/src/op.rs` — see
// `oidc_flow_live.rs`'s own module doc for why (a binary crate with no
// `lib.rs` can't be imported from an integration test in another crate).
// No key-rotation/refresh loop here: unlike that file, this suite never
// rotates a key mid-test, so a one-shot JWKS snapshot at startup is
// enough. ---

#[derive(Clone)]
struct OpState {
    store: Arc<sms_auth::op::MachineOnlyOpStore>,
    tokens: Arc<TokenManager>,
    config: OpConfig,
    jwks: Arc<Vec<Jwk>>,
}

impl FromRef<OpState> for Result<Arc<dyn OpStore>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.store.clone() as Arc<dyn OpStore>)
    }
}

impl FromRef<OpState> for Result<Arc<TokenManager>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.tokens.clone())
    }
}

impl FromRef<OpState> for OpConfig {
    fn from_ref(state: &OpState) -> Self {
        state.config.clone()
    }
}

async fn jwks_handler(State(state): State<OpState>) -> Json<JwksResponse> {
    Json(JwksResponse::new((*state.jwks).clone()))
}

async fn discovery_handler(State(state): State<OpState>) -> Json<OidcDiscovery> {
    Json(OidcDiscovery::from_config(&state.config).with_private_key_jwt())
}

fn op_router(state: OpState) -> Router {
    Router::new()
        .route("/jwks.json", get(jwks_handler))
        .route("/.well-known/openid-configuration", get(discovery_handler))
        .route("/token", post(axum_token_handler::<OpState>))
        .with_state(state)
}

/// Build the RSA keypair + JWK JSON a `private_key_jwt` client registers.
fn generate_client_keypair() -> (RsaPrivateKey, String) {
    let mut rng = rand::rngs::OsRng;
    let key = RsaPrivateKey::new(&mut rng, 2048).expect("generating a 2048-bit RSA key");
    let public = key.to_public_key();
    let n = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
    let e = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
    let jwks = serde_json::json!({
        "keys": [{"kty": "RSA", "kid": "client-key-1", "n": n, "e": e}]
    })
    .to_string();
    (key, jwks)
}

#[derive(serde::Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    jti: String,
    exp: i64,
}

fn sign_client_assertion(client_id: &str, issuer: &str, key: &RsaPrivateKey) -> String {
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .expect("encoding the client's private key to PEM");
    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes())
        .expect("building an EncodingKey from the freshly generated PEM");

    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("client-key-1".to_owned());

    let claims = AssertionClaims {
        iss: client_id,
        sub: client_id,
        aud: issuer,
        jti: unique_suffix(),
        exp: (Utc::now() + Duration::minutes(1)).timestamp(),
    };

    jsonwebtoken::encode(&header, &claims, &encoding_key).expect("signing the client assertion")
}

/// An `App` + one active `AppClient`/`OauthClient` registered for both
/// `sms:send` and `sms:read` — broad enough that this suite can request
/// either scope, or neither, from the real `/token` endpoint and see the
/// real consequence.
async fn seed_app_and_client(db: &Cratestack, jwks_json: &str) -> String {
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

    let client_id = format!("rbac-layer2-test-{suffix}");
    db.app_client()
        .create(schema::CreateAppClientInput {
            appId: app.id.clone(),
            clientId: client_id.clone(),
            label: "rbac layer2 test client".to_owned(),
            scopes: " sms:send sms:read ".to_owned(),
            lastUsedAt: None,
            retiredAt: None,
        })
        .run(&sys())
        .await
        .expect("seeding the app client");

    db.oauth_client()
        .create(schema::CreateOauthClientInput {
            clientId: client_id.clone(),
            appClientId: None,
            tokenEndpointAuthMethod: schema::ClientAuthMethod::private_key_jwt,
            jwks: Some(jwks_json.to_owned()),
            grantTypes: " client_credentials ".to_owned(),
            scopes: " sms:send sms:read ".to_owned(),
            redirectUris: " ".to_owned(),
            requirePkce: false,
        })
        .run(&sys())
        .await
        .expect("seeding the oauth client");

    client_id
}

/// An active `SenderId` with an `approved` registration against a fresh
/// `Provider` row — `sendMessage` needs the former to accept a send at
/// all, and this suite's `provider:update` route assertions need a real,
/// existing row to `PATCH` against rather than a made-up id (a 403 on a
/// row that actually exists is the meaningful assertion; a 403 that could
/// just as easily be a 404 in disguise is not). Returns `(senderIdValue,
/// providerId)`.
async fn seed_sender_and_provider(db: &Cratestack) -> (String, String) {
    let suffix = unique_suffix();
    let value = format!("T{}", &suffix[..suffix.len().min(9)]).to_uppercase();

    let provider = db
        .provider()
        .create(schema::CreateProviderInput {
            key: format!(
                "test_{}",
                suffix.to_lowercase().chars().take(20).collect::<String>()
            ),
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
        })
        .run(&owner())
        .await
        .expect("seeding a provider");

    let sender = db
        .sender_id()
        .create(schema::CreateSenderIdInput {
            value: value.clone(),
            kind: "alphanumeric".to_owned(),
            notes: None,
        })
        .run(&owner())
        .await
        .expect("seeding a sender id");

    db.sender_id_registration()
        .create(schema::CreateSenderIdRegistrationInput {
            senderIdId: sender.id.clone(),
            providerId: provider.id.clone(),
            status: "approved".to_owned(),
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
        .run(&owner())
        .await
        .expect("activating the sender id");

    (value, provider.id)
}

/// Rotates in a signing key, builds the OP's three routes plus
/// `sms_api::router` (which already carries #24's `enforce_route_permission`
/// wrapping `PATCH /providers/{id}` — see `router::PROVIDER_WRITE_ROUTES`),
/// and serves them on an OS-assigned loopback port.
async fn spawn_test_server(db: &Cratestack) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding an ephemeral port");
    let addr = listener.local_addr().expect("reading the bound address");
    let issuer = format!("http://{addr}");

    sms_auth::op::rotate_signing_key(db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a first signing key");
    let (signing, jwks) = sms_auth::op::load_signing_keys(db, &sys(), &issuer)
        .await
        .expect("loading the signing key just rotated in");

    let op_store = sms_auth::op::machine_only_store(Arc::new(db.clone()), sys());
    let op_config = sms_auth::op::machine_only_config(issuer.clone());
    let op_state = OpState {
        store: Arc::new(op_store),
        tokens: signing,
        config: op_config,
        jwks: Arc::new(jwks),
    };

    let auth = GatewayAuth::new(db.clone(), format!("{issuer}/jwks.json"), issuer.clone());
    let app = sms_api::router(db.clone(), auth).merge(op_router(op_state));

    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("serving the test app");
    });

    issuer
}

/// A `HashMap` also proves `RwLock` isn't needed here — unlike
/// `oidc_flow_live.rs`, nothing in this suite rotates a key mid-test, so
/// `spawn_test_server` hands back a plain, immutable state.
struct TestServer {
    issuer: String,
    client_id: String,
    client_key: RsaPrivateKey,
    sender_id: String,
    provider_id: String,
}

async fn setup() -> TestServer {
    let db = db().await;
    let issuer = spawn_test_server(&db).await;
    let (client_key, client_jwks_json) = generate_client_keypair();
    let client_id = seed_app_and_client(&db, &client_jwks_json).await;
    let (sender_id, provider_id) = seed_sender_and_provider(&db).await;
    TestServer {
        issuer,
        client_id,
        client_key,
        sender_id,
        provider_id,
    }
}

/// Exchange the client's `private_key_jwt` assertion for a token. `scope`
/// is passed through to the real `/token` endpoint verbatim — `None`
/// omits the form field entirely (not an empty string), matching how a
/// caller that never asks for a scope behaves. Returns the raw JSON
/// response so callers can assert on both the HTTP status and body.
async fn request_token(server: &TestServer, scope: Option<&str>) -> serde_json::Value {
    let assertion = sign_client_assertion(&server.client_id, &server.issuer, &server.client_key);
    let mut form = vec![
        ("grant_type", "client_credentials"),
        ("client_id", server.client_id.as_str()),
        (
            "client_assertion_type",
            "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
        ),
        ("client_assertion", assertion.as_str()),
    ];
    if let Some(scope) = scope {
        form.push(("scope", scope));
    }

    let response = reqwest::Client::new()
        .post(format!("{}/token", server.issuer))
        .form(&form)
        .send()
        .await
        .expect("POSTing to /token");
    assert!(
        response.status().is_success(),
        "token request itself must succeed (this suite only varies the granted scope, not \
         whether a token is issued): {}",
        response.status()
    );
    response
        .json()
        .await
        .expect("parsing the token response as JSON")
}

fn access_token(token_response: &serde_json::Value) -> &str {
    token_response["access_token"]
        .as_str()
        .expect("token response carries access_token")
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn send_message_denies_a_token_with_no_scope_at_all() {
    let server = setup().await;
    let token_response = request_token(&server, None).await;
    // §5.2, verbatim: "an omitted scope yields scope: None" — confirmed
    // against the real OP's own response, not assumed.
    assert!(
        token_response.get("scope").is_none() || token_response["scope"].is_null(),
        "expected no scope to be granted when none was requested: {token_response}"
    );

    let response = reqwest::Client::new()
        .post(format!("{}/$procs/sendMessage", server.issuer))
        .bearer_auth(access_token(&token_response))
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"args": {
            "to": unique_mtn_msisdn(),
            "body": "should never send",
            "senderId": server.sender_id,
        }}))
        .send()
        .await
        .expect("calling sendMessage");

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let body: serde_json::Value = response.json().await.expect("parsing the error body");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("sms:send"),
        "expected the denial to name the missing permission: {body}"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn send_message_denies_a_token_scoped_for_something_else() {
    let server = setup().await;
    let token_response = request_token(&server, Some("sms:read")).await;

    let response = reqwest::Client::new()
        .post(format!("{}/$procs/sendMessage", server.issuer))
        .bearer_auth(access_token(&token_response))
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"args": {
            "to": unique_mtn_msisdn(),
            "body": "should never send",
            "senderId": server.sender_id,
        }}))
        .send()
        .await
        .expect("calling sendMessage");

    assert_eq!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN,
        "a token scoped only for sms:read must not be able to send"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn send_message_succeeds_for_a_token_carrying_sms_send() {
    let server = setup().await;
    let token_response = request_token(&server, Some("sms:send")).await;
    assert_eq!(
        token_response["scope"].as_str(),
        Some("sms:send"),
        "the OP must echo back exactly the scope it granted: {token_response}"
    );

    let response = reqwest::Client::new()
        .post(format!("{}/$procs/sendMessage", server.issuer))
        .bearer_auth(access_token(&token_response))
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"args": {
            "to": unique_mtn_msisdn(),
            "body": "a real send, gated by a real sms:send scope",
            "senderId": server.sender_id,
        }}))
        .send()
        .await
        .expect("calling sendMessage");

    let status = response.status();
    let body: serde_json::Value = response.json().await.expect("parsing the response body");
    assert!(
        status.is_success(),
        "a correctly-scoped token must be able to send: {status}: {body}"
    );
    assert!(
        body["messageId"].as_str().is_some(),
        "a successful send returns the persisted message's id: {body}"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn provider_write_route_rejects_an_unauthenticated_request() {
    let server = setup().await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/providers/{}",
            server.issuer, server.provider_id
        ))
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("calling PATCH /providers/{id} with no bearer token");

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn provider_write_route_denies_a_token_with_no_scope_at_all() {
    let server = setup().await;
    let token_response = request_token(&server, None).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/providers/{}",
            server.issuer, server.provider_id
        ))
        .bearer_auth(access_token(&token_response))
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"maxTps": 10.0}))
        .send()
        .await
        .expect("calling PATCH /providers/{id}");

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let body: serde_json::Value = response.json().await.expect("parsing the error body");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("provider:update"),
        "expected the denial to name the missing permission: {body}"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn provider_write_route_denies_a_token_that_is_correctly_scoped_for_something_else() {
    // The point of this test: a token this suite already proved *works*
    // for sendMessage — a real, granted `sms:send` scope, not a missing
    // or malformed one — still cannot touch `PATCH /providers/{id}`.
    // Layer 2 narrows; it never widens (§5.1), and `sms:send` granting
    // access to `sendMessage` must never spill over into granting
    // anything else.
    let server = setup().await;
    let token_response = request_token(&server, Some("sms:send")).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/providers/{}",
            server.issuer, server.provider_id
        ))
        .bearer_auth(access_token(&token_response))
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"maxTps": 10.0}))
        .send()
        .await
        .expect("calling PATCH /providers/{id}");

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_route_this_middleware_does_not_gate_is_unaffected() {
    // `enforce_route_permission` is a no-op for any request that doesn't
    // match `PROVIDER_WRITE_ROUTES` — a scope-less token must still be
    // able to reach `previewMessage` (`@allow(auth() != null)`, no
    // permission requirement at all) exactly as it could before #24.
    let server = setup().await;
    let token_response = request_token(&server, None).await;

    let response = reqwest::Client::new()
        .post(format!("{}/$procs/previewMessage", server.issuer))
        .bearer_auth(access_token(&token_response))
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"args": {"body": "hello"}}))
        .send()
        .await
        .expect("calling previewMessage");

    assert!(
        response.status().is_success(),
        "previewMessage carries no permission requirement and must be unaffected by #24: {}",
        response.status()
    );
}
