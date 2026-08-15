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
//! - **The physical address `/token` is POSTed to can differ from the
//!   `issuer` identity the signed assertion asserts**, via
//!   [`PrivateKeyJwtConfig::connect_url`]. `issuer` alone always drives
//!   `aud` (and the `token_endpoint` REST/discovery clients would derive
//!   from it) — that identity is what the server's own OIDC configuration
//!   expects to see, and changing it changes what the server accepts, not
//!   just where the request goes. `connect_url` changes *only* the wire
//!   address this store's own `reqwest::Client` connects to, for
//!   topologies where the two genuinely differ: a container reachable at
//!   one address (a host port-forward, an ingress) while its own canonical
//!   issuer is a different one (an internal DNS name the caller can't
//!   resolve, or shouldn't need to). Defaults to `None`, meaning connect
//!   and identity are the same address — the common case, and every
//!   existing caller's behaviour is unchanged by this field's existence.

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
    /// The gateway's canonical OIDC issuer identity — what the signed
    /// client assertion's `aud` claim asserts (per `audience`), and, by
    /// default, where `/token` is physically POSTed too (matches
    /// `examples/rust/sms-send`'s own `--issuer`). Use
    /// [`PrivateKeyJwtConfig::connect_url`] when the network address this
    /// process can actually reach differs from this identity.
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
    /// Override for the network address `/token` is physically POSTed to,
    /// when it differs from `issuer`'s own `{issuer}/token` — see this
    /// module's own doc for the topology this exists for (a container
    /// reachable at a different address than its canonical issuer
    /// identity). Does **not** change `aud`: the signed assertion still
    /// asserts `issuer` (per `audience`), matching what the server's own
    /// OIDC configuration expects regardless of which address the request
    /// physically travels over. `None` (the default) POSTs to
    /// `{issuer}/token`, unchanged from this field not existing.
    pub connect_url: Option<String>,
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
            .field("connect_url", &self.connect_url)
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
            connect_url: None,
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
            connect_url: None,
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

    /// Builder-style override for [`PrivateKeyJwtConfig::connect_url`] —
    /// POST `/token` at this address instead of `{issuer}/token`, without
    /// changing what identity the signed assertion asserts. See this
    /// module's own doc for the topology this exists for.
    pub fn with_connect_url(mut self, connect_url: impl Into<String>) -> Self {
        self.connect_url = Some(connect_url.into());
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
    // The physical wire address `request_token` POSTs to — `{issuer}/token`
    // unless `PrivateKeyJwtConfig::connect_url` overrides it. Deliberately
    // *not* what `aud` is derived from; see `new`'s own comment on why the
    // two must stay independent.
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
        // `aud` is always derived from `issuer`, never from `connect_url` —
        // it is the identity the server's own OIDC configuration expects to
        // see asserted, independent of which address the request happens to
        // travel over. Computed before `token_endpoint` below so a future
        // edit can't accidentally make `aud` depend on the (possibly
        // overridden) physical address.
        let canonical_token_endpoint = format!("{issuer}/token");
        let aud = match config.audience {
            TokenAudience::TokenEndpoint => canonical_token_endpoint.clone(),
            TokenAudience::Issuer => issuer,
        };
        // The physical wire address, which *can* differ from the identity
        // above — see `PrivateKeyJwtConfig::connect_url`'s own doc.
        let token_endpoint = config.connect_url.unwrap_or(canonical_token_endpoint);
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

#[cfg(test)]
mod connect_url_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Throwaway 2048-bit RSA keypair, generated locally for these tests
    // only (`openssl genrsa 2048` / `openssl rsa -pubout`) — never used
    // against any real server, never registered against any real
    // `AppClient`. Only load-bearing property: the public key lets the
    // test verify a signed assertion's claims without needing to decode a
    // JWT by hand.
    const TEST_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
        MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQDb9rdZk91dYwDb\n\
        /vSm5qeY8gfEjtiJCv6cxD9zB2UslNQZtcMzIeh5T+MZBMmbHjZQ+I7RXSlvTcTl\n\
        rwECywnRQfEx7Ctqg2HlE2G+ryJokVpjob302ij2J7LTlBW22IRscJ3MFNmcQWmt\n\
        y+brq4TWOVxJ/y9Cy1CxfLg85+Y5XkMJBdWJfuJWSlsKEKT7nKmCvAeSyujspQaQ\n\
        XHsqQ0BJ9Ro7MvalBDv1rDs/r0YLadQUSvv/GONXKsja+OTVzjQPvtTfAFlT5260\n\
        xi17pWsv5ui44yKNhku8rXC0ntG9W3C2LFAEwTy4VL8+UDRNwI6r4pD94sdrXCVk\n\
        XBLTpWPHAgMBAAECggEAB2W4rM6IN0fJE5zzZVmEcWRSVo3nQlSYU3VWHOw2vtUS\n\
        fPrb4bBWPR4uqPy8Ovc9JGP3iZr0dcXLxV2pTwq8/ciV7/PdUeuxMx7/voXtRk79\n\
        FzStSrq7feu/29WzFT8a1PrEk8RvvQ2GHE4rKenOwdWUAPkQAdMMl6t2FrZFy9AL\n\
        vUTq6/fYmGRLJ/ioD5gTD11LYGzJ0rJva57uUZGEV9Z3899/CP3AS2PXpCacdlOT\n\
        gdWy5qwLwaytYVnIKZYeJNNcdgia+WQfHYyQFa3AKeq7+rKK9Z1c8Itw4tdwcSyo\n\
        piupk8L2ceeou1ds9wRAGiEJBlT3rADLjiv4lUaXAQKBgQD6UOG1X8yDPna0tyEi\n\
        Kaotdg8UvIYEF9xd4KxyVVB/LnQm8IJxMrTittQFSdrCXKuc+M/fU8eHKHWnGg1h\n\
        d8tNz2oZs1aPjW55EHUTIeMxlYOGVse0q0pLx0B97vmpe3ieTujjij/cEttLTNC8\n\
        iKXys5GseOoFxrzW5G+divD5YQKBgQDg9WSvKOkhRbhKTOrcdfBRXt7O3MEl77OB\n\
        a5TcosocemwROCtOGfFZHOmDjQgDzlUAHpfVE4NBf/bAaiKgPxUM78dXHhDQgzWI\n\
        Q7cgmESQ/QN0VjLuj/YC/UedBMtWpaY190GYOADjkMmGLeEAFOAruP84OQJHzXQm\n\
        Mtp7pMsmJwKBgAX4RdnIhh0fMT+QGocxDTD2Xte0w1F3rDbE8/fqFvhhiD2hgMro\n\
        Va4OhDH4F/KIuUMOIA8IdXrAuUWZ9nW7oKqjZrlkMI1N5zOV8+TX6w40raVXjn29\n\
        lBEImH4oY+Xp3u+PnDhJBMrf3EEIfPXyIFsQc0n0vEgU/E33tr3AIY0hAoGAR2IW\n\
        /O4CbChvfkRCEorqIyfzk7jBIYSadWrp5clSUQ8X0677LuFUkG54OuI5tNt4ZX1P\n\
        uLFkaRht+Ei1jBv4Vg6QNri3pTK9feve5FztBQUEX5oqt0C/U9uDKfQnges2ftFi\n\
        4yaCQbPj/sv4Jcp6B+XANtsAOkCbprNkWO/F9ukCgYA0tMYmG0Cxb0Icte2J47VQ\n\
        kw6qRQ1WIeoriYbBAj/M8dsTMroBR/sOIfn4XLJNeJcagtjFD6c2d+WaXsBxduuf\n\
        wZzKZT2PkKYn4S72b5Rmm0oBiwV15Po61rmfrIDJEmgs83d19Xm9ggmgJfsaxecw\n\
        EAuyvRydgzY44PKC0CKoPg==\n\
        -----END PRIVATE KEY-----\n";

    const TEST_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----\n\
        MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA2/a3WZPdXWMA2/70puan\n\
        mPIHxI7YiQr+nMQ/cwdlLJTUGbXDMyHoeU/jGQTJmx42UPiO0V0pb03E5a8BAssJ\n\
        0UHxMewraoNh5RNhvq8iaJFaY6G99Noo9iey05QVttiEbHCdzBTZnEFprcvm66uE\n\
        1jlcSf8vQstQsXy4POfmOV5DCQXViX7iVkpbChCk+5ypgrwHksro7KUGkFx7KkNA\n\
        SfUaOzL2pQQ79aw7P69GC2nUFEr7/xjjVyrI2vjk1c40D77U3wBZU+dutMYte6Vr\n\
        L+bouOMijYZLvK1wtJ7RvVtwtixQBME8uFS/PlA0TcCOq+KQ/eLHa1wlZFwS06Vj\n\
        xwIDAQAB\n\
        -----END PUBLIC KEY-----\n";

    #[derive(serde::Deserialize)]
    struct DecodedAssertionClaims {
        aud: String,
    }

    fn test_config(issuer: &str) -> PrivateKeyJwtConfig {
        PrivateKeyJwtConfig::new(
            issuer,
            "test-client",
            TEST_PRIVATE_KEY_PEM.as_bytes().to_vec(),
            "sms:send",
        )
    }

    /// Verifies the signed assertion against the matching test public key
    /// (proving it really is a genuine, correctly-signed assertion, not
    /// just any string) and returns its `aud` claim.
    fn decode_assertion_aud(assertion: &str) -> String {
        let key = jsonwebtoken::DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY_PEM.as_bytes())
            .expect("test public key parses");
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.validate_aud = false; // this test reads `aud` itself rather than asserting one through the validator
        validation.required_spec_claims.clear(); // this test's decode struct only declares `aud`
        jsonwebtoken::decode::<DecodedAssertionClaims>(assertion, &key, &validation)
            .expect("assertion verifies against the matching test key")
            .claims
            .aud
    }

    fn client_assertion_from_form_body(body: &[u8]) -> String {
        url::form_urlencoded::parse(body)
            .find(|(key, _)| key == "client_assertion")
            .map(|(_, value)| value.into_owned())
            .expect("client_assertion is present in the form-encoded /token body")
    }

    /// The property `connect_url` exists for: the physical POST goes to
    /// the override, while the signed assertion still asserts the
    /// canonical `issuer` identity — never the override — matching what
    /// the server's own OIDC configuration expects to see regardless of
    /// which address the request travels over.
    #[tokio::test]
    async fn connect_url_overrides_the_physical_target_but_not_the_asserted_audience() {
        let mock = MockServer::start().await;
        let canonical_issuer = "https://issuer.example.invalid";
        let config =
            test_config(canonical_issuer).with_connect_url(format!("{}/token", mock.uri()));

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "test-access-token",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&mock)
            .await;

        let store = PrivateKeyJwtTokenStore::new(config).expect("valid config");
        let token = store.get_token().await.expect("token exchange succeeds");
        assert_eq!(token, "test-access-token");

        let requests = mock
            .received_requests()
            .await
            .expect("recording is on by default");
        assert_eq!(
            requests.len(),
            1,
            "the physical request must land on connect_url exactly once"
        );

        let assertion = client_assertion_from_form_body(&requests[0].body);
        let aud = decode_assertion_aud(&assertion);
        assert_eq!(
            aud, "https://issuer.example.invalid/token",
            "aud must stay derived from `issuer`, never from `connect_url` — decoupling the \
             wire address from the asserted identity is the entire point of this field"
        );
    }

    /// Regression guard for the pre-existing default path: with no
    /// `connect_url`, behaviour must be exactly what it was before this
    /// field existed — POST `{issuer}/token`.
    #[tokio::test]
    async fn connect_url_none_falls_back_to_issuer_token_unchanged() {
        let mock = MockServer::start().await;
        // No override: the mock server's own address plays `issuer`
        // directly, matching every caller today.
        let config = test_config(&mock.uri());

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "default-path-token",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&mock)
            .await;

        let store = PrivateKeyJwtTokenStore::new(config).expect("valid config");
        let token = store.get_token().await.expect("token exchange succeeds");
        assert_eq!(token, "default-path-token");
    }
}
