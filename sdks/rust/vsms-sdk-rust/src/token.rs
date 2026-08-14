//! The `private_key_jwt` credential lifecycle (RFC 7523 §3 + OAuth2
//! `client_credentials`) — the one part of this SDK with no generated
//! counterpart, and the part every hand-rolled integration in this repo
//! has had to get right on its own: `examples/rust/sms-send`,
//! `frontends/packages/gateway/src/token.ts`, and
//! `backends/apps/sms-gateway/tests/provision_client_cli_live_postgres.rs` each
//! implement this dance separately today. This module is the one place
//! it should live.
//!
//! Four things below are load-bearing, each one this repo has already
//! learned the hard way (see issue #171 and `AGENTS.md`'s M1 section):
//!
//! - **`jti` is a fresh UUID on every assertion**, never reused.
//!   `ClientAssertion` is an insert-only table that replay-protects on
//!   this value at the database (a `23505` unique-constraint violation on
//!   `record_jti`), so resending the same assertion on a retry would
//!   collide with the original attempt rather than repeating it —
//!   `sign_assertion` mints a new `jti` on every call, so a retry
//!   naturally regenerates the whole assertion.
//! - **Tokens are cached with a 60-second expiry margin**, not minted
//!   per-call — matching `frontends/packages/gateway/src/token.ts`'s own
//!   `EXPIRY_SAFETY_MARGIN_SECONDS` exactly, so a request never starts
//!   with a token that expires before the response comes back.
//! - **`aud` accepts either the token endpoint URL or the bare issuer**
//!   (authkestra 0.3.2+ — see `TokenAudience`). The reference
//!   implementations (`token.ts`, `examples/rust/sms-send`) both send the
//!   token endpoint URL; that stays this crate's default so there is one
//!   fewer axis to debug if an exchange ever fails, but the choice is
//!   exposed rather than hardcoded.
//! - **The private key is never logged, and never reachable through a
//!   derived `Debug`.** `PrivateKeyJwtTokenStore` writes its own `Debug`
//!   impl that omits the signing key entirely — the same pattern
//!   `backends/crates/sms-api/src/pepper.rs`'s `HashPepper` uses in the main vsms
//!   repo, for the same reason: a derived `Debug` on a struct that holds
//!   this would leak it through any `{:?}` logging call site upstream of
//!   this crate, not just ones written here.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::SdkError;

/// Client assertions are meant to be short-lived — long enough to reach
/// `/token`, never long enough to be useful if intercepted in transit.
/// Matches `frontends/packages/gateway/src/token.ts`'s own `ASSERTION_TTL_SECONDS`.
const ASSERTION_TTL_SECONDS: i64 = 60;

/// Mint a fresh access token this many seconds before the cached one
/// actually expires, so a request never starts with a token that dies
/// mid-flight. Matches `token.ts`'s own `EXPIRY_SAFETY_MARGIN_SECONDS`.
const EXPIRY_SAFETY_MARGIN_SECONDS: i64 = 60;

/// Used when the token response omits `expires_in` (optional in the
/// OAuth2 response shape). Matches `token.ts`'s own fallback.
const DEFAULT_TOKEN_TTL_SECONDS: i64 = 15 * 60;

const CLIENT_ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is set before the Unix epoch")
        .as_secs() as i64
}

/// What to send as the client assertion's `aud` claim. `authkestra-op`
/// 0.3.2+ accepts either form (see `AGENTS.md`'s M1 section); this only
/// picks which one this SDK sends, not which ones the server will accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenAudience {
    /// `POST {issuer}/token` — the reference implementations' choice
    /// (`token.ts`, `examples/rust/sms-send`). Default.
    #[default]
    TokenEndpoint,
    /// The bare issuer URL, with no `/token` suffix.
    Issuer,
}

/// A trait owning "give me a valid Bearer token right now" and "the one I
/// gave you was rejected, throw it away" — deliberately smaller than
/// `cratestack::client_rust::RequestAuthorizer`, which is the wire-level
/// seam this plugs into (see `authorizer.rs`). Kept as its own trait
/// rather than folding straight into `RequestAuthorizer` so
/// [`VsmsClient`](crate::VsmsClient)'s bounded-refresh-on-401 logic has
/// something to call `invalidate` on without downcasting a trait object.
///
/// `#[async_trait]` (rather than native AFIT) for the same reason
/// `cratestack::client_rust::RequestAuthorizer` itself uses it (see that
/// trait's own doc, cratestack issue #453): a real credential provider
/// needs to make an HTTP call on a cache miss, and this trait has to
/// support `Arc<dyn TokenStore>` for that call to be awaited from inside
/// another dyn-dispatched async trait (`GatewayAuthorizer::authorize`).
#[async_trait::async_trait]
pub trait TokenStore: Send + Sync {
    /// A valid Bearer access token, minting and caching a new one only
    /// when the cached one is within its expiry margin. Concurrent
    /// callers during a cache miss serialize on the same mint rather than
    /// each minting their own token (and burning their own `jti`)
    /// simultaneously — see `PrivateKeyJwtTokenStore::get_token`'s own
    /// doc for how.
    async fn get_token(&self) -> Result<String, SdkError>;

    /// Drops the cached token, if any. [`VsmsClient`](crate::VsmsClient)
    /// calls this once on an unexpected 401 before retrying exactly once
    /// — the cache's own expiry margin should make that unreachable in
    /// normal operation, but a signing-key rotation invalidating the
    /// cached token mid-window is real, and an unbounded refresh loop
    /// against `/token` (which has no rate limiting today — #156) is a
    /// self-inflicted denial of service this SDK must not cause.
    async fn invalidate(&self);
}

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    /// Unix seconds; the cache is considered stale at or after this.
    expires_at: i64,
}

/// Configuration for [`PrivateKeyJwtTokenStore`]. Constructed once per
/// client; the private key is read (or supplied) here and turned into a
/// signing key immediately, so nothing after construction ever holds the
/// raw PEM bytes longer than it has to.
pub struct PrivateKeyJwtConfig {
    /// The gateway's externally reachable origin — both the OIDC issuer
    /// and `/token` hang off this (matches `examples/rust/sms-send`'s
    /// `--issuer`).
    pub issuer: String,
    /// The client id `sms-gateway provision-client` printed.
    pub client_id: String,
    /// The PEM-encoded RSA private key `sms-gateway provision-client`
    /// wrote. Taken as owned bytes so the caller decides how it was
    /// obtained (disk, a secrets manager, ...) — see
    /// [`PrivateKeyJwtConfig::from_key_path`] for the common disk case.
    /// Zeroed from this struct once consumed by
    /// [`PrivateKeyJwtTokenStore::new`]; never retained here longer than
    /// construction.
    pub private_key_pem: Vec<u8>,
    /// Space-separated scopes to request. Mandatory, not optional:
    /// omitting it does not fall back to the client's registered scopes,
    /// it mints a token with `scope: None`, and this deployment's Layer-2
    /// RBAC treats a missing scope as denial — the same footgun
    /// `token.ts`'s own module doc calls out.
    pub scope: String,
    /// Defaults to [`TokenAudience::TokenEndpoint`].
    pub audience: TokenAudience,
    /// Reuse an existing `reqwest::Client` (e.g. one already configured
    /// with a proxy or a custom timeout) rather than building a fresh one.
    /// `None` builds a default client backed by `rustls-no-provider` +
    /// `ring`, matching `cratestack::client_rust::CratestackClient`'s own
    /// TLS choice (see `client.rs`'s `ensure_crypto_provider`).
    pub http: Option<reqwest::Client>,
}

impl std::fmt::Debug for PrivateKeyJwtConfig {
    /// Hand-written, not derived — `private_key_pem` must never reach a
    /// `{:?}` log line. Same discipline as `PrivateKeyJwtTokenStore`'s own
    /// `Debug` below and `backends/crates/sms-api/src/pepper.rs`'s `HashPepper` in
    /// the main vsms repo.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateKeyJwtConfig")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("private_key_pem", &"<redacted>")
            .field("scope", &self.scope)
            .field("audience", &self.audience)
            .field("http", &"<reqwest::Client>")
            .finish()
    }
}

impl PrivateKeyJwtConfig {
    /// The common case: read the PEM `sms-gateway provision-client`
    /// wrote off disk. Never pass the key's *contents* on a command line
    /// or through an environment variable that ends up in process listing
    /// or shell history — only a path, matching
    /// `examples/rust/sms-send`'s own `--private-key-path` convention.
    pub fn from_key_path(
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        private_key_path: impl AsRef<Path>,
        scope: impl Into<String>,
    ) -> Result<Self, SdkError> {
        let path = private_key_path.as_ref();
        let private_key_pem = std::fs::read(path).map_err(|source| SdkError::PrivateKeyRead {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Self {
            issuer: issuer.into(),
            client_id: client_id.into(),
            private_key_pem,
            scope: scope.into(),
            audience: TokenAudience::default(),
            http: None,
        })
    }

    /// The private key supplied directly by the caller (already read from
    /// wherever it lives — a secrets manager, an env var populated by an
    /// orchestrator's own secret injection, ...).
    pub fn new(
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        private_key_pem: Vec<u8>,
        scope: impl Into<String>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            client_id: client_id.into(),
            private_key_pem,
            scope: scope.into(),
            audience: TokenAudience::default(),
            http: None,
        }
    }

    /// Builder-style override for [`TokenAudience`].
    pub fn with_audience(mut self, audience: TokenAudience) -> Self {
        self.audience = audience;
        self
    }

    /// Builder-style override to reuse an existing `reqwest::Client`.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }
}

#[derive(Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    jti: String,
    iat: i64,
    exp: i64,
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    // surfaced for callers who inspect responses in a debugger, not read here
    scope: Option<String>,
}

/// The default, and currently only, [`TokenStore`] implementation: OAuth2
/// `client_credentials` + `private_key_jwt` against `authkestra-op`'s
/// `/token`, exactly as `frontends/packages/gateway/src/token.ts` and
/// `examples/rust/sms-send` already did by hand — unified into one place.
pub struct PrivateKeyJwtTokenStore {
    http: reqwest::Client,
    token_endpoint: String,
    aud: String,
    client_id: String,
    scope: String,
    // Derived once from the caller-supplied PEM at construction time via
    // `jsonwebtoken::EncodingKey::from_rsa_pem`. `EncodingKey` holds key
    // material internally too, but never implements `Debug`/`Display` at
    // all, so there is no derived-`Debug` leak risk here the way there was
    // for the raw `Vec<u8>` in `PrivateKeyJwtConfig`.
    encoding_key: jsonwebtoken::EncodingKey,
    cached: Mutex<Option<CachedToken>>,
}

impl std::fmt::Debug for PrivateKeyJwtTokenStore {
    /// Hand-written, not derived — see this module's own doc and
    /// `PrivateKeyJwtConfig`'s `Debug` impl above for why.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateKeyJwtTokenStore")
            .field("token_endpoint", &self.token_endpoint)
            .field("aud", &self.aud)
            .field("client_id", &self.client_id)
            .field("scope", &self.scope)
            .field("encoding_key", &"<redacted>")
            .field(
                "cached",
                &self
                    .cached
                    .try_lock()
                    .map(|guard| guard.is_some())
                    .unwrap_or(true),
            )
            .finish()
    }
}

impl PrivateKeyJwtTokenStore {
    pub fn new(config: PrivateKeyJwtConfig) -> Result<Self, SdkError> {
        if config.scope.trim().is_empty() {
            return Err(SdkError::Config(
                "scope must not be empty — omitting it does not fall back to the client's \
                 registered scopes, it mints a token with `scope: None`, which this \
                 deployment's Layer-2 RBAC treats as denial"
                    .to_owned(),
            ));
        }
        let issuer = config.issuer.trim_end_matches('/').to_owned();
        let token_endpoint = format!("{issuer}/token");
        let aud = match config.audience {
            TokenAudience::TokenEndpoint => token_endpoint.clone(),
            TokenAudience::Issuer => issuer,
        };
        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(&config.private_key_pem)
            .map_err(|source| SdkError::PrivateKeyInvalid {
                path: "<supplied in-memory>".to_owned(),
                source,
            })?;
        let http = config.http.unwrap_or_else(|| {
            crate::client::ensure_crypto_provider();
            reqwest::Client::new()
        });
        Ok(Self {
            http,
            token_endpoint,
            aud,
            client_id: config.client_id,
            scope: config.scope,
            encoding_key,
            cached: Mutex::new(None),
        })
    }

    /// A fresh RFC 7523 §3 client assertion. `kid` is set to the client
    /// id — `authkestra_op`'s own `select_key` treats a single-key JWKS
    /// (which is all `provisionAppClient` ever produces, per AGENTS.md)
    /// as unambiguous even without a `kid`, but setting it costs nothing
    /// and matches what a real client should do. `jti` is a fresh UUID on
    /// every call — see this module's own doc for why that's load-
    /// bearing.
    fn sign_assertion(&self) -> Result<String, SdkError> {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(self.client_id.clone());
        let now = now_unix();
        let claims = AssertionClaims {
            iss: &self.client_id,
            sub: &self.client_id,
            aud: &self.aud,
            jti: uuid::Uuid::new_v4().to_string(),
            iat: now,
            exp: now + ASSERTION_TTL_SECONDS,
        };
        jsonwebtoken::encode(&header, &claims, &self.encoding_key)
            .map_err(SdkError::AssertionSigning)
    }

    async fn request_token(&self) -> Result<CachedToken, SdkError> {
        let assertion = self.sign_assertion()?;
        let response = self
            .http
            .post(&self.token_endpoint)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_assertion_type", CLIENT_ASSERTION_TYPE),
                ("client_assertion", assertion.as_str()),
                // Mandatory, not optional — see `PrivateKeyJwtTokenStore::new`'s
                // own empty-scope guard and this module's doc.
                ("scope", self.scope.as_str()),
            ])
            .send()
            .await
            .map_err(|error| SdkError::TokenRequest {
                endpoint: self.token_endpoint.clone(),
                message: error.to_string(),
            })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| SdkError::TokenRequest {
                endpoint: self.token_endpoint.clone(),
                message: format!("reading the response body: {error}"),
            })?;
        if !status.is_success() {
            return Err(SdkError::TokenRequest {
                endpoint: self.token_endpoint.clone(),
                message: format!("{status}: {body}"),
            });
        }
        let parsed: TokenResponse =
            serde_json::from_str(&body).map_err(|error| SdkError::TokenRequest {
                endpoint: self.token_endpoint.clone(),
                message: format!("parsing the response as JSON: {error}: {body}"),
            })?;

        let ttl = parsed.expires_in.unwrap_or(DEFAULT_TOKEN_TTL_SECONDS);
        Ok(CachedToken {
            access_token: parsed.access_token,
            expires_at: now_unix() + (ttl - EXPIRY_SAFETY_MARGIN_SECONDS).max(0),
        })
    }
}

#[async_trait::async_trait]
impl TokenStore for PrivateKeyJwtTokenStore {
    async fn get_token(&self) -> Result<String, SdkError> {
        // Held across the mint, not just the cache check: a second caller
        // arriving during a cache miss waits for the first caller's mint
        // to finish and reuses its result, rather than each minting (and
        // burning a `jti` on) its own token. This is the async
        // equivalent of `token.ts`'s shared `__vsmsGatewayTokenInFlight`
        // promise — a `Mutex` guarding the whole mint is a smaller
        // mechanism that gets the same "N concurrent callers, one
        // `/token` request" property.
        let mut guard = self.cached.lock().await;
        if let Some(cached) = guard.as_ref() {
            if cached.expires_at > now_unix() {
                return Ok(cached.access_token.clone());
            }
        }
        let token = self.request_token().await?;
        let access_token = token.access_token.clone();
        *guard = Some(token);
        Ok(access_token)
    }

    async fn invalidate(&self) {
        let mut guard = self.cached.lock().await;
        *guard = None;
    }
}
