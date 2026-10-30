#![doc = include_str!("lib.md")]

mod keys;

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Which of the fake's four throwaway keys signs a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SigningKey {
    /// RSA, published in the JWKS from the start.
    Rsa1,
    /// RSA, **not** published until [`FakeOidc::publish_key`] - models a
    /// rotation the relying party has not yet seen. Its JWK states no `alg`.
    Rsa2,
    /// RSA, never published by default - a token signed with it carries a
    /// signature no published key can verify.
    Rsa3,
    /// P-256 (for `ES256`), published from the start.
    Ec1,
}

impl SigningKey {
    /// The `kid` this key is published (and signs) under.
    #[must_use]
    pub fn kid(self) -> &'static str {
        match self {
            Self::Rsa1 => "fake-rsa-1",
            Self::Rsa2 => "fake-rsa-2",
            Self::Rsa3 => "fake-rsa-3",
            Self::Ec1 => "fake-ec-1",
        }
    }

    fn pem(self) -> &'static str {
        match self {
            Self::Rsa1 => keys::rsa(1),
            Self::Rsa2 => keys::rsa(2),
            Self::Rsa3 => keys::rsa(3),
            Self::Ec1 => keys::ec(),
        }
    }

    fn encoding_key(self) -> EncodingKey {
        match self {
            Self::Ec1 => EncodingKey::from_ec_pem(self.pem().as_bytes()),
            _ => EncodingKey::from_rsa_pem(self.pem().as_bytes()),
        }
        .expect("the embedded test key parses")
    }

    fn jwk(self) -> Value {
        let algorithm = match self {
            Self::Ec1 => Algorithm::ES256,
            _ => Algorithm::RS256,
        };
        let jwk = Jwk::from_encoding_key(&self.encoding_key(), algorithm)
            .expect("deriving the public JWK from a test key");
        let mut value = serde_json::to_value(jwk).expect("a JWK serialises");
        value["kid"] = json!(self.kid());
        value["use"] = json!("sig");
        // `Rsa2` publishes no `alg` (legal - the member is optional), so a
        // test can sign it with an algorithm other than the `RS256` the
        // other keys declare for themselves.
        if self == Self::Rsa2 {
            value
                .as_object_mut()
                .expect("a JWK is an object")
                .remove("alg");
        }
        value
    }
}

/// The JWS `alg` a scripted token is signed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigAlg {
    /// `RS256` - what Keycloak signs with by default.
    Rs256,
    /// `PS256`.
    Ps256,
    /// `ES256` (use with [`SigningKey::Ec1`]).
    Es256,
    /// `HS256` keyed with a secret derived from the public key - the
    /// classic algorithm-confusion attack.
    Hs256,
    /// `alg: none` - an unsigned token.
    None,
}

/// How the scripted token's `nonce` claim is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonceMode {
    /// Echo the `nonce` the authorization request carried - a correct `IdP`.
    FromRequest,
    /// A fixed value that is not the request's.
    Fixed(String),
    /// No `nonce` claim at all.
    Missing,
}

/// How the scripted token's header `kid` is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KidMode {
    /// The signing key's own `kid`.
    Own,
    /// This value instead - e.g. a published key's `kid` on a token signed
    /// by another key, or one nothing publishes.
    Override(String),
    /// No `kid` in the header.
    Missing,
}

/// Everything one scripted sign-in will put in its `id_token`. The default
/// is a well-formed token for an MFA-satisfied member of the staff group;
/// a test overrides exactly the field it wants to break.
#[derive(Debug, Clone)]
pub struct LoginSpec {
    /// `sub`.
    pub sub: String,
    /// `email`, if any.
    pub email: Option<String>,
    /// `email_verified`, if any, as the JSON boolean Keycloak sends. A test that
    /// wants a string or another shape puts it in [`Self::extra`], which is
    /// merged last.
    pub email_verified: Option<bool>,
    /// `name`, if any.
    pub name: Option<String>,
    /// `preferred_username`, if any.
    pub preferred_username: Option<String>,
    /// Group names, put under [`Self::groups_claim`]. `None` omits the claim.
    pub groups: Option<Vec<String>>,
    /// The claim the groups are published under (`groups` by default).
    pub groups_claim: String,
    /// `acr`, if any.
    pub acr: Option<String>,
    /// `amr`, if any.
    pub amr: Option<Vec<String>>,
    /// The `nonce` claim.
    pub nonce: NonceMode,
    /// `iss`; `None` means the issuer the fake serves.
    pub iss: Option<String>,
    /// `aud`; `None` means just the client id.
    pub aud: Option<Vec<String>>,
    /// `azp`, if any.
    pub azp: Option<String>,
    /// Seconds from now to `exp` (negative = already expired).
    pub exp_in_secs: i64,
    /// Seconds from now to `iat` (positive = issued in the future).
    pub iat_offset_secs: i64,
    /// `alg` of the signature.
    pub alg: SigAlg,
    /// Which key signs it.
    pub key: SigningKey,
    /// Header `kid`.
    pub kid: KidMode,
    /// Answer `/authorize` with `error=access_denied` instead of a code.
    pub deny: bool,
    /// Extra top-level claims merged in last.
    pub extra: serde_json::Map<String, Value>,
}

impl Default for LoginSpec {
    fn default() -> Self {
        Self {
            sub: "fake-sub-1".to_owned(),
            email: Some("sso.user@example.test".to_owned()),
            email_verified: Some(true),
            name: Some("SSO Test User".to_owned()),
            preferred_username: Some("sso.user".to_owned()),
            groups: Some(vec!["vsms-staff".to_owned()]),
            groups_claim: "groups".to_owned(),
            acr: None,
            amr: Some(vec!["pwd".to_owned(), "otp".to_owned()]),
            nonce: NonceMode::FromRequest,
            iss: None,
            aud: None,
            azp: None,
            exp_in_secs: 3600,
            iat_offset_secs: 0,
            alg: SigAlg::Rs256,
            key: SigningKey::Rsa1,
            kid: KidMode::Own,
            deny: false,
            extra: serde_json::Map::new(),
        }
    }
}

/// One request the fake's `/token` endpoint received.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // one flag per independent check the exchange makes
pub struct TokenRequest {
    /// Whether `client_secret_basic` credentials matched.
    pub client_authenticated: bool,
    /// The raw `Authorization` header value, for tests that assert the
    /// credential travelled as `Basic`, never in the body.
    pub authorization: Option<String>,
    /// Whether the body carried a `client_secret` (it must not).
    pub secret_in_body: bool,
    /// `grant_type`.
    pub grant_type: String,
    /// Whether `redirect_uri` equalled the one used at `/authorize`.
    pub redirect_uri_matched: bool,
    /// Whether `code_verifier` hashed to the stored S256 challenge.
    pub verifier_matched: bool,
    /// Whether the exchange succeeded.
    pub issued: bool,
}

struct CodeRecord {
    spec: LoginSpec,
    nonce: Option<String>,
    redirect_uri: String,
    code_challenge: String,
}

struct Inner {
    client_id: String,
    client_secret: String,
    issuer: String,
    discovery_issuer: Option<String>,
    published: Vec<SigningKey>,
    queue: VecDeque<LoginSpec>,
    codes: HashMap<String, CodeRecord>,
    next_code: u64,
    authorize_log: Vec<HashMap<String, String>>,
    token_log: Vec<TokenRequest>,
    jwks_fetches: usize,
    discovery_fetches: usize,
}

type Shared = Arc<Mutex<Inner>>;

fn lock(shared: &Shared) -> MutexGuard<'_, Inner> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running fake `IdP`, bound to an ephemeral loopback port. Dropping it
/// stops the server.
pub struct FakeOidc {
    shared: Shared,
    issuer: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeOidc {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeOidc {
    /// Starts the fake. `client_id`/`client_secret` are what its `/token`
    /// endpoint accepts under `client_secret_basic`.
    ///
    /// # Panics
    ///
    /// If no loopback port can be bound - a broken test environment.
    pub async fn start(client_id: &str, client_secret: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding an ephemeral loopback port");
        let addr: SocketAddr = listener.local_addr().expect("reading the bound address");
        let issuer = format!("http://127.0.0.1:{}/realms/staff", addr.port());
        let shared: Shared = Arc::new(Mutex::new(Inner {
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            issuer: issuer.clone(),
            discovery_issuer: None,
            published: vec![SigningKey::Rsa1, SigningKey::Ec1],
            queue: VecDeque::new(),
            codes: HashMap::new(),
            next_code: 0,
            authorize_log: Vec::new(),
            token_log: Vec::new(),
            jwks_fetches: 0,
            discovery_fetches: 0,
        }));
        let app = Router::new()
            .route(
                "/realms/staff/.well-known/openid-configuration",
                get(discovery),
            )
            .route("/realms/staff/jwks.json", get(jwks))
            .route("/realms/staff/authorize", get(authorize))
            .route("/realms/staff/token", post(token))
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            shared,
            issuer,
            task,
        }
    }

    /// The issuer this fake serves, e.g. `http://127.0.0.1:PORT/realms/staff`.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Queues the next sign-in. Each `/authorize` pops one; with the queue
    /// empty the fake signs [`LoginSpec::default`].
    pub fn queue_login(&self, spec: LoginSpec) {
        lock(&self.shared).queue.push_back(spec);
    }

    /// Adds `key` to the published JWKS (models a rotation landing).
    pub fn publish_key(&self, key: SigningKey) {
        let mut inner = lock(&self.shared);
        if !inner.published.contains(&key) {
            inner.published.push(key);
        }
    }

    /// Makes discovery claim a different `issuer` than the one the fake
    /// serves under - an issuer-mismatch scenario.
    pub fn set_discovery_issuer(&self, issuer: &str) {
        lock(&self.shared).discovery_issuer = Some(issuer.to_owned());
    }

    /// Every query-parameter set `/authorize` received, in order.
    #[must_use]
    pub fn authorize_requests(&self) -> Vec<HashMap<String, String>> {
        lock(&self.shared).authorize_log.clone()
    }

    /// Every `/token` request received, in order.
    #[must_use]
    pub fn token_requests(&self) -> Vec<TokenRequest> {
        lock(&self.shared).token_log.clone()
    }

    /// How many times the JWKS has been fetched.
    #[must_use]
    pub fn jwks_fetches(&self) -> usize {
        lock(&self.shared).jwks_fetches
    }

    /// How many times the discovery document has been fetched.
    #[must_use]
    pub fn discovery_fetches(&self) -> usize {
        lock(&self.shared).discovery_fetches
    }
}

async fn discovery(State(shared): State<Shared>) -> Response {
    let mut inner = lock(&shared);
    inner.discovery_fetches += 1;
    let base = inner.issuer.clone();
    let advertised = inner
        .discovery_issuer
        .clone()
        .unwrap_or_else(|| base.clone());
    axum::Json(json!({
        "issuer": advertised,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "jwks_uri": format!("{base}/jwks.json"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256", "ES256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"],
        "code_challenge_methods_supported": ["S256"],
    }))
    .into_response()
}

async fn jwks(State(shared): State<Shared>) -> Response {
    let mut inner = lock(&shared);
    inner.jwks_fetches += 1;
    let keys: Vec<Value> = inner.published.iter().map(|k| k.jwk()).collect();
    axum::Json(json!({ "keys": keys })).into_response()
}

fn plain(status: StatusCode, body: &str) -> Response {
    (status, body.to_owned()).into_response()
}

async fn authorize(
    State(shared): State<Shared>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let mut inner = lock(&shared);
    inner.authorize_log.push(params.clone());

    let get = |key: &str| params.get(key).map(String::as_str);
    if get("client_id") != Some(inner.client_id.as_str()) {
        return plain(StatusCode::BAD_REQUEST, "unknown client_id");
    }
    if get("response_type") != Some("code") {
        return plain(StatusCode::BAD_REQUEST, "response_type must be code");
    }
    let Some(redirect_uri) = get("redirect_uri") else {
        return plain(StatusCode::BAD_REQUEST, "redirect_uri is required");
    };
    let Some(code_challenge) = get("code_challenge") else {
        return plain(StatusCode::BAD_REQUEST, "code_challenge is required");
    };
    if get("code_challenge_method") != Some("S256") {
        return plain(
            StatusCode::BAD_REQUEST,
            "code_challenge_method must be S256",
        );
    }

    let spec = inner.queue.pop_front().unwrap_or_default();
    let mut query = form_urlencoded::Serializer::new(String::new());
    if spec.deny {
        query.append_pair("error", "access_denied");
    } else {
        inner.next_code += 1;
        let code = format!("fake-code-{}", inner.next_code);
        inner.codes.insert(
            code.clone(),
            CodeRecord {
                spec,
                nonce: get("nonce").map(str::to_owned),
                redirect_uri: redirect_uri.to_owned(),
                code_challenge: code_challenge.to_owned(),
            },
        );
        query.append_pair("code", &code);
    }
    if let Some(state) = get("state") {
        query.append_pair("state", state);
    }
    let separator = if redirect_uri.contains('?') { '&' } else { '?' };
    let location = format!("{redirect_uri}{separator}{}", query.finish());
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the epoch offset fits an i64")
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sign_id_token(
    spec: &LoginSpec,
    request_nonce: Option<&str>,
    issuer: &str,
    client_id: &str,
) -> String {
    let now = now_secs();
    let mut claims = serde_json::Map::new();
    claims.insert("iss".into(), json!(spec.iss.as_deref().unwrap_or(issuer)));
    claims.insert("sub".into(), json!(spec.sub));
    match &spec.aud {
        None => claims.insert("aud".into(), json!(client_id)),
        Some(aud) if aud.len() == 1 => claims.insert("aud".into(), json!(aud[0])),
        Some(aud) => claims.insert("aud".into(), json!(aud)),
    };
    if let Some(azp) = &spec.azp {
        claims.insert("azp".into(), json!(azp));
    }
    claims.insert("exp".into(), json!(now + spec.exp_in_secs));
    claims.insert("iat".into(), json!(now + spec.iat_offset_secs));
    match &spec.nonce {
        NonceMode::FromRequest => {
            if let Some(nonce) = request_nonce {
                claims.insert("nonce".into(), json!(nonce));
            }
        }
        NonceMode::Fixed(value) => {
            claims.insert("nonce".into(), json!(value));
        }
        NonceMode::Missing => {}
    }
    if let Some(email) = &spec.email {
        claims.insert("email".into(), json!(email));
    }
    if let Some(verified) = spec.email_verified {
        claims.insert("email_verified".into(), json!(verified));
    }
    if let Some(name) = &spec.name {
        claims.insert("name".into(), json!(name));
    }
    if let Some(username) = &spec.preferred_username {
        claims.insert("preferred_username".into(), json!(username));
    }
    if let Some(acr) = &spec.acr {
        claims.insert("acr".into(), json!(acr));
    }
    if let Some(amr) = &spec.amr {
        claims.insert("amr".into(), json!(amr));
    }
    if let Some(groups) = &spec.groups {
        claims.insert(spec.groups_claim.clone(), json!(groups));
    }
    for (key, value) in &spec.extra {
        claims.insert(key.clone(), value.clone());
    }
    let claims = Value::Object(claims);

    let kid = match &spec.kid {
        KidMode::Own => Some(spec.key.kid().to_owned()),
        KidMode::Override(kid) => Some(kid.clone()),
        KidMode::Missing => None,
    };

    match spec.alg {
        SigAlg::None => {
            let mut header = json!({ "alg": "none", "typ": "JWT" });
            if let Some(kid) = kid {
                header["kid"] = json!(kid);
            }
            format!(
                "{}.{}.",
                b64(header.to_string().as_bytes()),
                b64(claims.to_string().as_bytes())
            )
        }
        alg => {
            let (algorithm, key) = match alg {
                SigAlg::Rs256 => (Algorithm::RS256, spec.key.encoding_key()),
                SigAlg::Ps256 => (Algorithm::PS256, spec.key.encoding_key()),
                SigAlg::Es256 => (Algorithm::ES256, spec.key.encoding_key()),
                // The attack: HMAC keyed with material the attacker can
                // read - here the published key's modulus bytes.
                SigAlg::Hs256 => (
                    Algorithm::HS256,
                    EncodingKey::from_secret(spec.key.jwk().to_string().as_bytes()),
                ),
                SigAlg::None => unreachable!("handled above"),
            };
            let mut header = Header::new(algorithm);
            header.kid = kid;
            jsonwebtoken::encode(&header, &claims, &key).expect("signing a scripted id_token")
        }
    }
}

fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    // RFC 6749 2.3.1: both halves are form-urlencoded before being joined.
    let decode = |s: &str| {
        form_urlencoded::parse(format!("k={s}").as_bytes())
            .next()
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    };
    Some((decode(id), decode(secret)))
}

fn token_error(status: StatusCode, error: &str) -> Response {
    (status, axum::Json(json!({ "error": error }))).into_response()
}

async fn token(State(shared): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let mut inner = lock(&shared);
    let form: HashMap<String, String> = form_urlencoded::parse(&body).into_owned().collect();
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let client_authenticated = basic_credentials(&headers)
        .is_some_and(|(id, secret)| id == inner.client_id && secret == inner.client_secret);
    let mut record = TokenRequest {
        client_authenticated,
        authorization,
        secret_in_body: form.contains_key("client_secret"),
        grant_type: form.get("grant_type").cloned().unwrap_or_default(),
        redirect_uri_matched: false,
        verifier_matched: false,
        issued: false,
    };

    if !client_authenticated {
        inner.token_log.push(record);
        return token_error(StatusCode::UNAUTHORIZED, "invalid_client");
    }
    if record.grant_type != "authorization_code" {
        inner.token_log.push(record);
        return token_error(StatusCode::BAD_REQUEST, "unsupported_grant_type");
    }
    // Single use: the code is gone whether or not the rest checks out.
    let Some(code) = form.get("code").and_then(|c| inner.codes.remove(c)) else {
        inner.token_log.push(record);
        return token_error(StatusCode::BAD_REQUEST, "invalid_grant");
    };
    record.redirect_uri_matched = form.get("redirect_uri") == Some(&code.redirect_uri);
    record.verifier_matched = form
        .get("code_verifier")
        .is_some_and(|verifier| b64(&Sha256::digest(verifier.as_bytes())) == code.code_challenge);
    if !record.redirect_uri_matched || !record.verifier_matched {
        inner.token_log.push(record);
        return token_error(StatusCode::BAD_REQUEST, "invalid_grant");
    }

    let id_token = sign_id_token(
        &code.spec,
        code.nonce.as_deref(),
        &inner.issuer.clone(),
        &inner.client_id.clone(),
    );
    record.issued = true;
    inner.token_log.push(record);
    axum::Json(json!({
        "access_token": "fake-upstream-access-token",
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": id_token,
    }))
    .into_response()
}
