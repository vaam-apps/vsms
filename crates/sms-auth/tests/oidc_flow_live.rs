//! The actual gate for #20/#21: a real signing key, a real client keypair,
//! a real `private_key_jwt` client assertion, a real token minted by the
//! OP, and `GatewayAuth` validating it against the OP's own real,
//! HTTP-served JWKS — no part of this path is mocked.
//!
//! Builds the same three OP routes `app/sms-gateway/src/op.rs` does,
//! duplicated here rather than imported: `app/sms-gateway` is a binary
//! crate with no `lib.rs`, so its own modules aren't reachable from an
//! integration test in a different crate. The ~30 lines this duplicates
//! are wiring, not logic — the logic under test
//! (`sms_auth::op`/`sms_api::GatewayAuth`) is the real, shared code.
//! Future changes to the real `op.rs` (`discovery_handler` included) should
//! be mirrored here.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-auth --test oidc_flow_live -- --ignored
//! ```

use std::sync::{Arc, RwLock};
use std::time::Duration as StdDuration;

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
use sms_api::{GatewayAuth, HashPepper};

fn sys() -> CoolContext {
    Principal {
        sub: "oidc-flow-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner() -> CoolContext {
    Principal {
        sub: "oidc-flow-test-owner".to_owned(),
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

async fn db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

// --- The OP's routes, duplicated from app/sms-gateway/src/op.rs — see this
// file's own module doc for why. Including the live key refresh: this
// test's own rotation-overlap assertion exists specifically to prove that
// refresh actually reaches the served `/jwks.json`, not just the database
// `load_signing_keys` reads from (found live in review, #97 — the first
// version of this test asserted against the DB layer, which doesn't
// exercise the refresh loop at all). ---

/// Short relative to `app/sms-gateway`'s own `DEFAULT_KEY_REFRESH_INTERVAL`
/// (60s) so this test observes a refresh without a real production-length
/// wait.
const TEST_KEY_REFRESH_INTERVAL: StdDuration = StdDuration::from_millis(100);

#[derive(Clone)]
struct OpState {
    store: Arc<sms_auth::op::MachineOnlyOpStore>,
    tokens: Arc<RwLock<Arc<TokenManager>>>,
    config: OpConfig,
    jwks: Arc<RwLock<Arc<Vec<Jwk>>>>,
}

impl OpState {
    fn refresh(&self, tokens: Arc<TokenManager>, jwks: Vec<Jwk>) {
        *self
            .tokens
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = tokens;
        *self
            .jwks
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(jwks);
    }
}

impl FromRef<OpState> for Result<Arc<dyn OpStore>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state.store.clone() as Arc<dyn OpStore>)
    }
}

impl FromRef<OpState> for Result<Arc<TokenManager>, AxumError> {
    fn from_ref(state: &OpState) -> Self {
        Ok(state
            .tokens
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
}

impl FromRef<OpState> for OpConfig {
    fn from_ref(state: &OpState) -> Self {
        state.config.clone()
    }
}

async fn jwks_handler(State(state): State<OpState>) -> Json<JwksResponse> {
    let jwks = state
        .jwks
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    Json(JwksResponse::new((*jwks).clone()))
}

/// Mirrors `app/sms-gateway/src/op.rs`'s `discovery_handler` — see this
/// file's own module doc for why the duplication, and that file's doc for
/// why `authkestra_axum::op::axum_discovery_handler` can't be used as-is.
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

fn spawn_key_refresh(state: OpState, db: Cratestack, sys: CoolContext, issuer: String) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TEST_KEY_REFRESH_INTERVAL);
        loop {
            ticker.tick().await;
            if let Ok((tokens, jwks)) = sms_auth::op::load_signing_keys(&db, &sys, &issuer).await {
                state.refresh(tokens, jwks);
            }
        }
    });
}

/// Build the RSA keypair + JWK JSON a `private_key_jwt` client registers —
/// hand-rolled (`rsa`'s own crate has no JWK export) the same way
/// `sms_auth::op::rotate_signing_key` hand-rolls the OP's own key
/// generation.
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

/// A `private_key_jwt` client assertion per RFC 7523 §3 — `iss`/`sub` both
/// the `client_id`, `aud` the OP's issuer (accepted alongside the token
/// endpoint URL, per `authkestra_op::client_assertion::verify_client_assertion`'s
/// own doc), a fresh `jti`, `exp` a minute out.
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

async fn seed_app_and_client(db: &Cratestack, jwks_json: &str) -> String {
    let suffix = unique_suffix();
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "oidc flow test app".to_owned(),
            slug: format!("oidc-flow-test-{suffix}"),
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

    let client_id = format!("oidc-test-{suffix}");
    db.app_client()
        .create(schema::CreateAppClientInput {
            appId: app.id.clone(),
            clientId: client_id.clone(),
            label: "oidc flow test client".to_owned(),
            scopes: " sms:send ".to_owned(),
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

/// Rotates in a first signing key, builds the same three OP routes plus
/// `GatewayAuth` that `app/sms-gateway/src/main.rs` itself wires, and
/// serves them on an OS-assigned loopback port. Returns the issuer URL to
/// reach it at, and how many keys were already in JWKS at that point — the
/// baseline the rotation-overlap assertion measures growth against, since
/// this database is never reset between runs and an earlier run's
/// still-within-its-overlap-window key would make an absolute count flaky.
async fn spawn_test_server(db: &Cratestack) -> (String, usize) {
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
    let baseline_jwks_count = jwks.len();

    let op_store = sms_auth::op::machine_only_store(Arc::new(db.clone()), sys());
    let op_config = sms_auth::op::machine_only_config(issuer.clone());
    let op_state = OpState {
        store: Arc::new(op_store),
        tokens: Arc::new(RwLock::new(signing)),
        config: op_config,
        jwks: Arc::new(RwLock::new(Arc::new(jwks))),
    };
    spawn_key_refresh(op_state.clone(), db.clone(), sys(), issuer.clone());

    let auth = GatewayAuth::new(db.clone(), format!("{issuer}/jwks.json"), issuer.clone());
    // #134: this suite never sends a message, so — like
    // `provision_app_client_live_postgres.rs` — any pepper over the
    // minimum length works.
    let pepper = HashPepper::new("oidc-flow-live-test-pepper-well-over-the-minimum-length")
        .expect("test pepper meets HashPepper::new's minimum length");
    let app = sms_api::router(db.clone(), auth, pepper).merge(op_router(op_state));

    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("serving the test app");
    });

    (issuer, baseline_jwks_count)
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_real_private_key_jwt_flow_mints_a_token_gatewayauth_accepts() {
    let db = db().await;
    let (issuer, baseline_jwks_count) = spawn_test_server(&db).await;

    let (client_key, client_jwks_json) = generate_client_keypair();
    let client_id = seed_app_and_client(&db, &client_jwks_json).await;

    let assertion = sign_client_assertion(&client_id, &issuer, &client_key);
    let client = reqwest::Client::new();
    let token_response = client
        .post(format!("{issuer}/token"))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", &client_id),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            ("client_assertion", &assertion),
            ("scope", "sms:send"),
        ])
        .send()
        .await
        .expect("POSTing to /token");
    let status = token_response.status();
    let body: serde_json::Value = token_response
        .json()
        .await
        .expect("parsing the token response as JSON");
    assert!(
        status.is_success(),
        "token request failed ({status}): {body}"
    );
    let access_token = body["access_token"]
        .as_str()
        .expect("token response carries access_token");

    // The gap found while closing out #18: `private_key_jwt` must be
    // advertised in the discovery document, not just accepted at /token —
    // a spec-compliant client consults discovery to decide which
    // client-authentication method to use, and the request above already
    // proves the OP accepts an assertion, so a discovery document that
    // stayed silent about it would be a real interop gap, not just a
    // cosmetic one.
    let discovery: serde_json::Value = client
        .get(format!("{issuer}/.well-known/openid-configuration"))
        .send()
        .await
        .expect("fetching /.well-known/openid-configuration")
        .json()
        .await
        .expect("parsing the discovery document as JSON");
    let auth_methods = discovery["token_endpoint_auth_methods_supported"]
        .as_array()
        .expect("token_endpoint_auth_methods_supported is a JSON array");
    assert!(
        auth_methods
            .iter()
            .any(|method| method == "private_key_jwt"),
        "discovery document must advertise private_key_jwt in \
         token_endpoint_auth_methods_supported: {discovery}"
    );

    // GatewayAuth accepts the token for a procedure call — the whole
    // validate-then-project chain works end to end.
    let preview = client
        .post(format!("{issuer}/$procs/previewMessage"))
        .bearer_auth(access_token)
        // The router also serves CBOR and negotiates on `Accept` — without
        // this, an unset `Accept` apparently resolves to CBOR rather than
        // JSON, and this test's own `.json()` response parsing below fails
        // on content type, not on anything GatewayAuth or previewMessage
        // itself did.
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({"args": {"body": "hello from the live OIDC flow test"}}))
        .send()
        .await
        .expect("calling previewMessage");
    assert!(
        preview.status().is_success(),
        "previewMessage was rejected ({}) with a token GatewayAuth should have accepted: {}",
        preview.status(),
        preview.text().await.unwrap_or_default()
    );

    // The P0 this rebuild exists to close: role "app" must never reach
    // OauthSigningKey, which is gated hasRole('system') only. This is a
    // "list" route, and CrateStack's policy enforcement on list/find_many
    // reads is row-level filtering, not a request-level 403 — a denied
    // caller gets `200 OK` with zero rows, the same shape as a caller who
    // asked correctly and nothing matched. Confirmed live: the request
    // itself succeeds, and the array is empty.
    let signing_keys = client
        .get(format!("{issuer}/oauth_signing_keys"))
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .expect("calling GET /oauth_signing_keys");
    assert_eq!(signing_keys.status(), reqwest::StatusCode::OK);
    let signing_keys_body: Vec<serde_json::Value> = signing_keys
        .json()
        .await
        .expect("parsing the oauth_signing_keys response as a JSON array");
    assert!(
        signing_keys_body.is_empty(),
        "a machine token must never see any OauthSigningKey row: got {signing_keys_body:?}"
    );

    // Rotation overlap, asserted against the actual served /jwks.json —
    // not just the database `load_signing_keys` itself reads from. An
    // earlier version of this test asserted the DB-layer result directly,
    // which doesn't exercise the running server's own refresh loop at
    // all — found live in review (#97) alongside the underlying bug: the
    // first version of app/sms-gateway's op.rs captured JWKS once at
    // startup and never refreshed it, so a rotation against a live server
    // silently never took effect until a restart. `spawn_key_refresh`
    // (both here and in app/sms-gateway/src/op.rs) is what this now
    // proves actually works, end to end over real HTTP.
    //
    // Measured as "count grows by exactly one" rather than an absolute
    // "== 2" — this database is never reset between runs, so earlier
    // runs' still-within-their-overlap-window keys are legitimately still
    // present and would make an absolute count flaky, not the growth this
    // assertion actually cares about.
    sms_auth::op::rotate_signing_key(&db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a second signing key");
    tokio::time::sleep(TEST_KEY_REFRESH_INTERVAL * 3).await;
    let jwks_after_second_rotation: JwksResponse = client
        .get(format!("{issuer}/jwks.json"))
        .send()
        .await
        .expect("fetching /jwks.json after a second rotation")
        .json()
        .await
        .expect("parsing /jwks.json as JSON");
    assert_eq!(
        jwks_after_second_rotation.keys.len(),
        baseline_jwks_count + 1,
        "the first key must still publish during its rotation-overlap window, not just the \
         newly active one — and the running server must actually have refreshed to see it, \
         not just the database"
    );
}
