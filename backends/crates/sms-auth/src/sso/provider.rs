//! The upstream `OpenID` Provider as this gateway sees it: discovery, the
//! signing keys, the authorization URL, and the back-channel code exchange.
//!
//! **One issuer, and only that one.** Discovery is fetched from the
//! configured issuer and refused unless its `issuer` equals the configured
//! one byte for byte (OIDC Discovery 4.3 - the defence against a mix-up
//! where a document claims to be someone else). The JWKS is fetched only
//! from the `jwks_uri` that document names, and a `kid` that is not in it
//! earns **one** refetch - never more often than
//! [`SsoConfig::jwks_min_refresh`] - so an attacker spraying unknown `kid`s
//! cannot turn this broker into a request amplifier against the `IdP`.
//!
//! **A dead `IdP` must not take the gateway down.** Discovery is therefore
//! fetched lazily and cached, not demanded at process start: password
//! sign-in is the documented break-glass path and has to keep working when
//! Keycloak is the thing that is broken. A fetch failure surfaces as
//! [`SsoError::Unavailable`] on the sign-in that needed it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{Algorithm, decode_header};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use url::Url;

use super::config::{SsoConfig, require_secure_url};
use super::error::{Refusal, SsoError};
use super::id_token::{self, Expected, IdClaims};

/// Discovery documents and JWKS are small; anything larger is not one.
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// How long a discovery document is trusted before it is fetched again.
const METADATA_TTL: Duration = Duration::from_secs(3600);
/// How long a JWKS is trusted before it is fetched again even when every
/// `kid` in sight is known (so a key the `IdP` has withdrawn stops verifying).
const JWKS_TTL: Duration = Duration::from_secs(3600);

/// The per-transaction secrets of the upstream authorization request. The
/// `state` is the only one that ever reaches the browser, and the database
/// stores only its hash.
#[derive(Debug, Clone)]
pub struct AuthnSecrets {
    /// Opaque, 256 bits.
    pub state: String,
    /// Opaque, 192 bits; echoed back inside the `id_token`.
    pub nonce: String,
    /// The PKCE verifier (RFC 7636), 256 bits.
    pub code_verifier: String,
    /// `BASE64URL(SHA256(code_verifier))`.
    pub code_challenge: String,
}

impl AuthnSecrets {
    /// Fresh random values from the operating system's CSPRNG.
    #[must_use]
    pub fn generate() -> Self {
        fn token(bytes: usize) -> String {
            let mut buf = vec![0u8; bytes];
            OsRng.fill_bytes(&mut buf);
            URL_SAFE_NO_PAD.encode(buf)
        }
        let code_verifier = token(32);
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        Self {
            state: token(32),
            nonce: token(24),
            code_verifier,
            code_challenge,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    token_endpoint_auth_methods_supported: Option<Vec<String>>,
    code_challenge_methods_supported: Option<Vec<String>>,
}

#[derive(Debug)]
struct Metadata {
    authorization_endpoint: Url,
    token_endpoint: Url,
    jwks_uri: Url,
}

struct CachedMetadata {
    metadata: Arc<Metadata>,
    fetched: Instant,
}

#[derive(Default)]
struct JwksCache {
    keys: Vec<Jwk>,
    fetched: Option<Instant>,
}

impl JwksCache {
    fn find(&self, kid: &str) -> Option<Jwk> {
        self.keys
            .iter()
            .find(|key| key.common.key_id.as_deref() == Some(kid))
            .cloned()
    }
}

struct Inner {
    config: SsoConfig,
    http: reqwest::Client,
    /// The caches. Held only to read or to store, **never across a network call**.
    metadata: Mutex<Option<CachedMetadata>>,
    jwks: Mutex<JwksCache>,
    /// Single flight: at most one task fetches discovery, and one the JWKS, at a
    /// time. A caller that finds one in flight and holds something stale uses what
    /// it has instead of queueing behind an identity provider that may take the
    /// whole request timeout to fail.
    metadata_flight: Mutex<()>,
    jwks_flight: Mutex<()>,
    metadata_ttl: Duration,
    jwks_ttl: Duration,
}

/// A handle to the upstream `IdP`. Cheap to clone.
#[derive(Clone)]
pub struct SsoProvider {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SsoProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SsoProvider")
            .field("issuer", &self.inner.config.issuer)
            .finish_non_exhaustive()
    }
}

fn unavailable(what: &str) -> SsoError {
    SsoError::Unavailable(what.to_owned())
}

/// Reads at most [`MAX_BODY_BYTES`] of a response body.
async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, SsoError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| unavailable("reading an upstream response failed"))?
    {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(unavailable("an upstream response was too large"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

impl SsoProvider {
    /// Builds the provider and its HTTP client: no redirects followed (a
    /// token or JWKS endpoint that redirects is misconfigured or hostile),
    /// bounded connect and total time.
    ///
    /// # Errors
    ///
    /// The HTTP client could not be constructed.
    pub fn new(config: SsoConfig) -> Result<Self, SsoError> {
        Self::with_cache_lifetimes(config, METADATA_TTL, JWKS_TTL)
    }

    /// As [`Self::new`], with the two cache lifetimes chosen. Production uses
    /// [`Self::new`] (an hour each); a test that needs a cache to have expired
    /// without waiting an hour uses this.
    ///
    /// # Errors
    ///
    /// The HTTP client could not be constructed.
    #[doc(hidden)]
    pub fn with_cache_lifetimes(
        config: SsoConfig,
        metadata_ttl: Duration,
        jwks_ttl: Duration,
    ) -> Result<Self, SsoError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("vsms-sms-auth/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| unavailable("building the SSO HTTP client failed"))?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                http,
                metadata: Mutex::new(None),
                jwks: Mutex::new(JwksCache::default()),
                metadata_flight: Mutex::new(()),
                jwks_flight: Mutex::new(()),
                metadata_ttl,
                jwks_ttl,
            }),
        })
    }

    /// The validated configuration.
    #[must_use]
    pub fn config(&self) -> &SsoConfig {
        &self.inner.config
    }

    async fn fetch_discovery(&self) -> Result<Metadata, SsoError> {
        let config = &self.inner.config;
        let url = format!(
            "{}/.well-known/openid-configuration",
            config.issuer.trim_end_matches('/')
        );
        let response = self
            .inner
            .http
            .get(&url)
            .send()
            .await
            .map_err(|_| unavailable("fetching the discovery document failed"))?;
        if !response.status().is_success() {
            tracing::error!(status = %response.status(), "SSO discovery answered with a failure status");
            return Err(unavailable("the discovery document was not served"));
        }
        let body = read_limited(response).await?;
        let document: Discovery = serde_json::from_slice(&body)
            .map_err(|_| unavailable("the discovery document is not valid"))?;

        if document.issuer != config.issuer {
            // Byte for byte, and not a refusal of the user: the `IdP` (or
            // whatever answered for it) is claiming to be somebody else.
            tracing::error!(
                configured = %config.issuer,
                "SSO discovery `issuer` does not match the configured issuer; refusing to use it"
            );
            return Err(unavailable(
                "the discovery issuer does not match the configured one",
            ));
        }
        if let Some(methods) = &document.token_endpoint_auth_methods_supported
            && !methods.iter().any(|m| m == "client_secret_basic")
        {
            return Err(unavailable("the IdP does not offer client_secret_basic"));
        }
        if let Some(methods) = &document.code_challenge_methods_supported
            && !methods.iter().any(|m| m == "S256")
        {
            return Err(unavailable("the IdP does not offer PKCE S256"));
        }
        let parse = |field: &'static str, value: &str| {
            let url = require_secure_url(field, value).map_err(|error| {
                tracing::error!(%error, "SSO discovery named an unacceptable endpoint");
                unavailable("the discovery document names an unacceptable endpoint")
            })?;
            // Under livemode the issuer is https (the loader says so), and so is
            // every endpoint its discovery document names: plain http to loopback
            // is a developer's Keycloak, never a document that points a production
            // gateway's code exchange at `http://127.0.0.1`.
            if self.inner.config.livemode && url.scheme() != "https" {
                tracing::error!(
                    field,
                    "SSO discovery named a cleartext endpoint under livemode"
                );
                return Err(unavailable(
                    "the discovery document names a cleartext endpoint",
                ));
            }
            Ok(url)
        };
        Ok(Metadata {
            authorization_endpoint: parse(
                "authorization_endpoint",
                &document.authorization_endpoint,
            )?,
            token_endpoint: parse("token_endpoint", &document.token_endpoint)?,
            jwks_uri: parse("jwks_uri", &document.jwks_uri)?,
        })
    }

    async fn metadata(&self) -> Result<Arc<Metadata>, SsoError> {
        let ttl = self.inner.metadata_ttl;
        let cached = {
            let slot = self.inner.metadata.lock().await;
            slot.as_ref()
                .map(|cached| (cached.metadata.clone(), cached.fetched.elapsed()))
        };
        if let Some((metadata, age)) = &cached
            && *age < ttl
        {
            return Ok(metadata.clone());
        }
        // Stale or absent. One fetch at a time; a caller with something stale
        // never waits for it.
        let _flight = match (self.inner.metadata_flight.try_lock(), &cached) {
            (Ok(flight), _) => flight,
            (Err(_), Some((metadata, _))) => return Ok(metadata.clone()),
            (Err(_), None) => self.inner.metadata_flight.lock().await,
        };
        // Whoever held the flight may have just filled it.
        if let Some(cached) = self.inner.metadata.lock().await.as_ref()
            && cached.fetched.elapsed() < ttl
        {
            return Ok(cached.metadata.clone());
        }
        match self.fetch_discovery().await {
            Ok(fresh) => {
                let metadata = Arc::new(fresh);
                *self.inner.metadata.lock().await = Some(CachedMetadata {
                    metadata: metadata.clone(),
                    fetched: Instant::now(),
                });
                Ok(metadata)
            }
            Err(error) => {
                // A stale document beats none: the `IdP` being briefly down
                // must not stop sign-ins that need only what we already know.
                if let Some((metadata, _)) = cached {
                    tracing::warn!(%error, "SSO discovery refresh failed; using the cached document");
                    return Ok(metadata);
                }
                Err(error)
            }
        }
    }

    /// The upstream authorization URL for one transaction.
    ///
    /// # Errors
    ///
    /// [`SsoError::Unavailable`] when discovery cannot be had.
    pub async fn authorization_url(
        &self,
        secrets: &AuthnSecrets,
        prompt_login: bool,
    ) -> Result<Url, SsoError> {
        let metadata = self.metadata().await?;
        let config = &self.inner.config;
        let mut url = metadata.authorization_endpoint.clone();
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("response_type", "code")
                .append_pair("client_id", &config.client_id)
                .append_pair("redirect_uri", &config.redirect_uri)
                .append_pair("scope", &config.scopes)
                .append_pair("state", &secrets.state)
                .append_pair("nonce", &secrets.nonce)
                .append_pair("code_challenge", &secrets.code_challenge)
                .append_pair("code_challenge_method", "S256");
            if prompt_login {
                query.append_pair("prompt", "login");
            }
        }
        Ok(url)
    }

    /// Exchanges an authorization code for the `id_token` (the only thing
    /// the broker uses from the response), authenticating with
    /// `client_secret_basic` - RFC 6749 2.3.1, both halves form-urlencoded
    /// before being joined - and the PKCE verifier.
    ///
    /// # Errors
    ///
    /// [`Refusal::UpstreamDenied`] when the `IdP` rejects the code (`400`:
    /// spent, expired or mismatched - the user's problem);
    /// [`SsoError::Unavailable`] for everything else, a wrong client secret
    /// included (the operator's problem).
    pub async fn exchange_code(&self, code: &str, code_verifier: &str) -> Result<String, SsoError> {
        let metadata = self.metadata().await?;
        let config = &self.inner.config;
        let encode = |value: &str| {
            url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
        };
        let credentials = STANDARD.encode(format!(
            "{}:{}",
            encode(&config.client_id),
            encode(config.client_secret.expose())
        ));
        let response = self
            .inner
            .http
            .post(metadata.token_endpoint.clone())
            .header("Authorization", format!("Basic {credentials}"))
            .header("Accept", "application/json")
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", config.redirect_uri.as_str()),
                ("code_verifier", code_verifier),
            ])
            .send()
            .await
            .map_err(|_| unavailable("the code exchange could not reach the IdP"))?;
        let status = response.status();
        let body = read_limited(response).await?;
        if status == reqwest::StatusCode::BAD_REQUEST {
            let error = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_default();
            tracing::warn!(%error, "the IdP rejected the authorization code");
            return Err(Refusal::UpstreamDenied.into());
        }
        if !status.is_success() {
            // 401 here is `invalid_client`: the configured secret is wrong
            // or revoked. Loud, because only an operator can fix it.
            tracing::error!(%status, "the IdP's token endpoint answered with a failure status");
            return Err(unavailable("the IdP's token endpoint failed"));
        }
        serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("id_token").and_then(Value::as_str).map(str::to_owned))
            .ok_or_else(|| unavailable("the token response carried no id_token"))
    }

    /// Fetches the published keys. Touches no cache: the caller stores the result
    /// only if it is good.
    async fn fetch_jwks(&self) -> Result<Vec<Jwk>, SsoError> {
        let metadata = self.metadata().await?;
        let response = self
            .inner
            .http
            .get(metadata.jwks_uri.clone())
            .send()
            .await
            .map_err(|_| unavailable("fetching the JWKS failed"))?;
        if !response.status().is_success() {
            return Err(unavailable("the JWKS was not served"));
        }
        let body = read_limited(response).await?;
        let document: Value =
            serde_json::from_slice(&body).map_err(|_| unavailable("the JWKS is not valid JSON"))?;
        let keys = document
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| unavailable("the JWKS has no `keys` array"))?;
        // One unparseable key (an encryption key of a kind this library does
        // not model) must not discard the rest.
        Ok(keys
            .iter()
            .filter_map(|key| serde_json::from_value::<Jwk>(key.clone()).ok())
            .filter(|key| key.common.key_id.is_some())
            .collect())
    }

    /// The key a token's header names.
    ///
    /// A key younger than the TTL is used as is. Otherwise the keys are fetched
    /// again, at most once per `jwks_min_refresh` and by one task at a time, and
    /// **a stale key beats none**: if the fetch fails, or another task is already
    /// making it, a key held from an earlier fetch is used. (A key the provider has
    /// since withdrawn stops verifying when a fetch *succeeds* without it.) No lock
    /// is held across the network call.
    async fn find_key(&self, kid: &str) -> Result<Jwk, SsoError> {
        let min_refresh = self.inner.config.jwks_min_refresh;
        let (known, fetched) = {
            let cache = self.inner.jwks.lock().await;
            (cache.find(kid), cache.fetched)
        };
        if let (Some(key), Some(at)) = (&known, fetched)
            && at.elapsed() < self.inner.jwks_ttl
        {
            return Ok(key.clone());
        }
        if fetched.is_some_and(|at| at.elapsed() < min_refresh) {
            // Refetching is rate-limited: better the key we have than none.
            return known.ok_or_else(|| Refusal::UnknownKeyId.into());
        }
        let _flight = match (self.inner.jwks_flight.try_lock(), &known) {
            (Ok(flight), _) => flight,
            (Err(_), Some(key)) => return Ok(key.clone()),
            (Err(_), None) => self.inner.jwks_flight.lock().await,
        };
        {
            // The task that held the flight may have just fetched.
            let cache = self.inner.jwks.lock().await;
            if cache.fetched.is_some_and(|at| at.elapsed() < min_refresh) {
                return cache.find(kid).ok_or_else(|| Refusal::UnknownKeyId.into());
            }
        }
        match self.fetch_jwks().await {
            Ok(keys) => {
                let mut cache = self.inner.jwks.lock().await;
                cache.keys = keys;
                cache.fetched = Some(Instant::now());
                cache.find(kid).ok_or_else(|| Refusal::UnknownKeyId.into())
            }
            Err(error) => match known {
                Some(key) => {
                    tracing::warn!(%error, "SSO JWKS refresh failed; using the cached key");
                    Ok(key)
                }
                None => Err(error),
            },
        }
    }

    /// Verifies `raw` as an `id_token` for this transaction's `nonce`.
    ///
    /// # Errors
    ///
    /// A [`Refusal`] for anything wrong with the token itself;
    /// [`SsoError::Unavailable`] when the keys could not be fetched.
    pub async fn verify_id_token(&self, raw: &str, nonce: &str) -> Result<IdClaims, SsoError> {
        // Look at the raw `alg` name first: `none` (and any other name the
        // JWT library cannot model) would otherwise surface as a generic
        // parse error, and "unsigned token" is the case worth naming.
        if let Some(name) = id_token::peek_algorithm_name(raw)
            && !id_token::ALLOWED_ALGORITHM_NAMES.contains(&name.as_str())
        {
            return Err(Refusal::AlgorithmNotAllowed.into());
        }
        let header = decode_header(raw).map_err(|_| Refusal::IdTokenMalformed)?;
        let alg: Algorithm = header.alg;
        if !id_token::is_allowed_algorithm(alg) {
            return Err(Refusal::AlgorithmNotAllowed.into());
        }
        let kid = header.kid.ok_or(Refusal::MissingKeyId)?;
        let jwk = self.find_key(&kid).await?;
        let config = &self.inner.config;
        Ok(id_token::verify(
            raw,
            &jwk,
            alg,
            &Expected {
                issuer: &config.issuer,
                client_id: &config.client_id,
                nonce,
                groups_claim: &config.groups_claim,
            },
        )?)
    }
}
