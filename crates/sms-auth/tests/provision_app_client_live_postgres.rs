//! `provisionAppClient` (#23) against a real, fully migrated Postgres.
//!
//! Two things this suite exists to prove that no unit test can:
//!
//! 1. The `AppClient` + `OauthClient` rows `Procedures::provision_app_client`
//!    writes actually persist, in the same transaction, correctly linked
//!    (`OauthClient.appClientId -> AppClient.id`) — not just that the
//!    in-memory `ProvisionClientResult` it returns looks right.
//! 2. The private key handed back once in that result is not a
//!    look-alike: it actually builds a `private_key_jwt` client assertion
//!    that the real OP `/token` endpoint
//!    (`authkestra_op::client_assertion::verify_client_assertion`, reached
//!    through `authkestra_axum::op::axum_token_handler`) accepts against the
//!    public JWK `provision_app_client` persisted into `OauthClient.jwks`.
//!    If the JSON shape written there ever drifted from what
//!    `verify_client_assertion`'s `select_key` expects (a JWK Set,
//!    `{"keys": [...]}`), this is what would catch it — `create_inputs.rs`'s
//!    compile-time coverage cannot, since it only proves the *input struct*
//!    accepts a well-formed `jwks` string, not that
//!    `provision_app_client` writes one.
//!
//! Deliberately narrow, per this ticket's own scope: whether `GatewayAuth`
//! accepts the *access token* `/token` returns for a resource call is
//! already covered end-to-end by `oidc_flow_live.rs`'s hand-seeded client.
//! Building #25's full acceptance gate (process restart, a
//! developer-role-refused assertion, ...) is that later, separate ticket's
//! job, not this one's.
//!
//! Route wiring duplicated from `oidc_flow_live.rs` rather than shared —
//! see that file's own module doc for why (`app/sms-gateway` is a binary
//! crate with no importable `lib.rs`). Only `/token` is mounted here: this
//! suite has no need for `/jwks.json` or discovery, both already covered
//! there.
//!
//! `sms_test_support` provisions Postgres and applies both migrations
//! automatically (a shared, self-healing container — see its own module
//! doc), so running this needs only Docker and:
//!
//! ```bash
//! cargo test -p sms-auth --test provision_app_client_live_postgres -- --ignored
//! ```

use std::sync::Arc;

use authkestra_axum::helpers::AxumError;
use authkestra_axum::op::axum_token_handler;
use authkestra_engine::TokenManager;
use authkestra_op::config::OpConfig;
use authkestra_op::OpStore;
use axum::extract::FromRef;
use axum::routing::post;
use axum::Router;
use chrono::{Duration, Utc};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::CoolContext;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, procedures::provision_app_client, procedures::ProcedureRegistry, ClientAuthMethod,
    Cratestack,
};
use sms_api::{HashPepper, Procedures};

/// #102, found live: on a genuinely fresh database, this binary's own
/// tests — run concurrently by Rust's default multi-threaded test
/// harness — can race on Postgres's own `pg_type` catalog the first time
/// two of them prepare the exact same not-yet-cached query shape at the
/// same instant. See `crates/sms-worker/tests/claim_live_postgres.rs`'s
/// own `TEST_MUTEX` doc for the full reasoning — same mechanism, same
/// fix. `oidc_flow_live.rs` doesn't need this: it never has two tests in
/// the same binary racing to prepare the same query shape for the first
/// time, since it only has the one live-server test.
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn owner() -> CoolContext {
    Principal {
        sub: "provision-app-client-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CoolContext {
    Principal {
        sub: "provision-app-client-test-system".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// #134: `provision_app_client` never hashes anything — only `sendMessage`
/// does — so this suite has no stake in *which* pepper `Procedures` holds,
/// only that it holds a valid one. Any fixed value over the minimum length
/// works.
fn test_pepper() -> HashPepper {
    HashPepper::new("provision-app-client-live-postgres-test-pepper-over-the-minimum")
        .expect("test pepper meets HashPepper::new's minimum length")
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
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

async fn seed_app(db: &Cratestack) -> schema::App {
    let suffix = unique_suffix();
    db.app()
        .create(schema::CreateAppInput {
            name: "provision app client test app".to_owned(),
            slug: format!("provision-test-{suffix}"),
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
}

// --- Just enough of the OP's routing to reach `/token` — see this file's
// own module doc for why only this one route is mounted here. ---

#[derive(Clone)]
struct OpState {
    store: Arc<sms_auth::op::MachineOnlyOpStore>,
    tokens: Arc<TokenManager>,
    config: OpConfig,
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

fn op_router(state: OpState) -> Router {
    Router::new()
        .route("/token", post(axum_token_handler::<OpState>))
        .with_state(state)
}

/// Rotates in a first signing key and serves `/token` on an OS-assigned
/// loopback port. Returns the issuer URL to reach it at.
async fn spawn_token_endpoint(db: &Cratestack) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding an ephemeral port");
    let addr = listener.local_addr().expect("reading the bound address");
    let issuer = format!("http://{addr}");

    sms_auth::op::rotate_signing_key(db, &sys(), sms_auth::op::ROTATION_OVERLAP)
        .await
        .expect("rotating in a first signing key");
    let (signing, _jwks) = sms_auth::op::load_signing_keys(db, &sys(), &issuer)
        .await
        .expect("loading the signing key just rotated in");

    let op_store = sms_auth::op::machine_only_store(Arc::new(db.clone()), sys());
    let op_config = sms_auth::op::machine_only_config(issuer.clone());
    let op_state = OpState {
        store: Arc::new(op_store),
        tokens: signing,
        config: op_config,
    };
    let app = op_router(op_state);

    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("serving the test /token endpoint");
    });

    issuer
}

#[derive(serde::Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    jti: String,
    exp: i64,
}

/// A `private_key_jwt` client assertion per RFC 7523 §3, signed with
/// exactly the PEM `provisionAppClient` returned — no `kid` in the header,
/// matching `select_key`'s "no kid means the jwks must hold exactly one
/// key" rule (vendored `authkestra-op` source, checked directly): a freshly
/// provisioned client's `jwks` always holds exactly one.
fn sign_client_assertion(client_id: &str, issuer: &str, private_key_pem: &str) -> String {
    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
        .expect("building an EncodingKey from the returned private key PEM");

    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    let claims = AssertionClaims {
        iss: client_id,
        sub: client_id,
        aud: issuer,
        jti: unique_suffix(),
        exp: (Utc::now() + Duration::minutes(1)).timestamp(),
    };

    jsonwebtoken::encode(&header, &claims, &encoding_key).expect("signing the client assertion")
}

fn provision_args(app_id: &str, label: &str, scopes: Vec<String>) -> provision_app_client::Args {
    provision_app_client::Args {
        args: schema::ProvisionClientInput {
            appId: app_id.to_owned(),
            label: label.to_owned(),
            scopes,
        },
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn provisioning_persists_linked_app_client_and_oauth_client_rows() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db).await;
    let procedures = Procedures::new(test_pepper());

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — the "sanctioned way to invoke a procedure from
    // non-HTTP code" per that function's own doc comment.
    let args = provision_args(&app.id, "otp service", vec!["sms:send".to_owned()]);
    // `&owner()` called twice in one statement would borrow a temporary that
    // doesn't outlive the closure's returned future (E0515) — bind it once
    // instead.
    let ctx = owner();
    let result = provision_app_client::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.provision_app_client(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("provisioning a well-formed request must succeed");

    assert!(
        result.clientId.len() >= 8,
        "clientId must satisfy @length(min: 8, max: 64): {}",
        result.clientId
    );
    assert!(result.privateKeyPem.contains("PRIVATE KEY"));

    // Read back under `system` — the same role `AppClient`/`OauthClient`
    // admit for read (see schema.cstack's comments on both models' own
    // `@@allow("read", ...)`).
    let app_client = db
        .app_client()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::app_client::clientId().eq(result.clientId.as_str()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading back the AppClient row")
        .into_iter()
        .next()
        .expect("provisioning must have created an AppClient row");
    assert_eq!(app_client.appId, app.id);
    assert_eq!(app_client.label, "otp service");
    assert!(app_client.active, "a freshly provisioned client is active");

    let oauth_client = db
        .oauth_client()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::oauth_client::clientId().eq(result.clientId.as_str()),
        ))
        .limit(1)
        .run(&sys())
        .await
        .expect("reading back the OauthClient row")
        .into_iter()
        .next()
        .expect("provisioning must have created an OauthClient row");
    assert_eq!(oauth_client.appClientId, Some(app_client.id));
    assert_eq!(
        oauth_client.tokenEndpointAuthMethod,
        ClientAuthMethod::private_key_jwt
    );

    let jwks: serde_json::Value = serde_json::from_str(
        oauth_client
            .jwks
            .as_deref()
            .expect("a private_key_jwt client must have jwks set"),
    )
    .expect("jwks must be valid JSON");
    let keys = jwks["keys"]
        .as_array()
        .expect("jwks must be a JWK Set (an object with a `keys` array)");
    assert_eq!(
        keys.len(),
        1,
        "a freshly provisioned client holds exactly one key: {jwks}"
    );
    assert_eq!(keys[0]["kty"], "RSA");
    assert!(
        keys[0].get("d").is_none(),
        "jwks must never hold the private exponent"
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_returned_private_key_builds_an_assertion_the_op_accepts() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let app = seed_app(&db).await;
    let procedures = Procedures::new(test_pepper());
    let issuer = spawn_token_endpoint(&db).await;

    // cratestack 0.7.13 (cratestack#512): see the identical comment on the
    // test above.
    let args = provision_args(
        &app.id,
        "token flow test client",
        vec!["sms:send".to_owned()],
    );
    // `&owner()` called twice in one statement would borrow a temporary that
    // doesn't outlive the closure's returned future (E0515) — bind it once
    // instead.
    let ctx = owner();
    let result = provision_app_client::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.provision_app_client(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("provisioning a well-formed request must succeed");

    let assertion = sign_client_assertion(&result.clientId, &issuer, &result.privateKeyPem);

    let client = reqwest::Client::new();
    let token_response = client
        .post(format!("{issuer}/token"))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", result.clientId.as_str()),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            ("client_assertion", assertion.as_str()),
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
        "the OP must accept an assertion signed with provisionAppClient's own returned key \
         ({status}): {body}"
    );
    assert!(
        body["access_token"].as_str().is_some(),
        "token response must carry access_token: {body}"
    );
}
