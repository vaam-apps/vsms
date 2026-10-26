//! A sealed (`COSE_Sign1`) HTTP client for the live suites.
//!
//! Since VS-3 every generated route answers only a sealed request, so a suite
//! that used to attach `Authorization: Bearer <token>` signs instead. This is
//! the one place that does it: [`TestKey`] is a throwaway signing key,
//! [`SealedClient`] seals a call with it, sends it, and opens the answer
//! against the server's pinned public key.
//!
//! It deliberately knows nothing about vsms's schema (this crate cannot depend
//! on `sms-api`, which dev-depends on it): the caller passes the generated
//! `OP_CONTRACTS` table, and registers the key's public half itself, through
//! `registerAppClientKey` / `register-client` for an app key or
//! `registerConsoleKey` / `pair-console-key` for a console key.
//!
//! ```no_run
//! # async fn demo(contracts: cratestack::OpContracts, server_public: [u8; 32]) {
//! use sms_test_support::sealed::{SealedClient, TestKey};
//!
//! let key = TestKey::app();
//! // ... register `key.public_key_b64u()` for an AppClient ...
//! let client = SealedClient::new("http://127.0.0.1:8080", &key, &[server_public], contracts);
//! let reply = client
//!     .request("POST", "/$procs/sendMessage")
//!     .json(&serde_json::json!({ "args": { "args": {} } }))
//!     .send()
//!     .await;
//! assert!(reply.sealed);
//! # }
//! ```

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cratestack::OpContracts;
use cratestack::axum::http::{HeaderMap, HeaderName, HeaderValue};
use cratestack::client_rust::{ClientEnvelope, RouteRef, SealCall, SealedCall};
use cratestack::cose::{
    CoseEnvelope, CoseMode, CoseSigner, CoseVerifyKey, Ed25519Signer, P256Signer,
    StaticVerifierResolver,
};

/// The audience every request binds; vsms's own constant, spelled out so a
/// suite fails if the server's changes.
pub const AUDIENCE: &str = "vsms";

/// The only payload type vsms seals.
pub const PAYLOAD_TYPE: &str = "application/json";

/// A signing key for a test: an Ed25519 app key or a P-256 console key.
#[derive(Clone)]
pub struct TestKey {
    signer: Arc<dyn CoseSigner>,
    verify: CoseVerifyKey,
}

fn random_32() -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(cratestack::uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(cratestack::uuid::Uuid::new_v4().as_bytes());
    bytes
}

impl TestKey {
    /// A fresh random Ed25519 key, the kind an app client signs with.
    #[must_use]
    pub fn app() -> Self {
        Self::app_from_seed(&random_32())
    }

    /// The Ed25519 key of this seed.
    #[must_use]
    pub fn app_from_seed(seed: &[u8; 32]) -> Self {
        let signer = Ed25519Signer::from_seed(seed);
        let verify = signer.verify_key();
        Self {
            signer: Arc::new(signer),
            verify,
        }
    }

    /// A fresh random P-256 key, the kind a console (a browser) signs with.
    ///
    /// # Panics
    ///
    /// Never in practice: the top bit of the scalar is cleared, which keeps
    /// it below the curve order.
    #[must_use]
    pub fn console() -> Self {
        let mut scalar = random_32();
        scalar[0] &= 0x7f;
        scalar[0] |= 0x01;
        let signer = P256Signer::from_scalar(&scalar).expect("a scalar below the curve order");
        let verify = signer.verify_key();
        Self {
            signer: Arc::new(signer),
            verify,
        }
    }

    /// The public key as unpadded base64url: the raw 32-byte Ed25519 point
    /// for an app key, the 65-byte SEC1 uncompressed point for a console key.
    /// What `registerAppClientKey` / `registerConsoleKey` and the CLIs take.
    ///
    /// # Panics
    ///
    /// Never: a [`TestKey`] always holds one of the two key kinds.
    #[must_use]
    pub fn public_key_b64u(&self) -> String {
        if let Some(bytes) = self.verify.ed25519_bytes() {
            URL_SAFE_NO_PAD.encode(bytes)
        } else {
            URL_SAFE_NO_PAD.encode(
                self.verify
                    .p256_sec1_uncompressed()
                    .expect("a TestKey is Ed25519 or P-256"),
            )
        }
    }

    /// The key's RFC 9679 thumbprint, lowercase hex: what `ClientKey` /
    /// `ConsoleKey` store as `thumbprint`.
    #[must_use]
    pub fn thumbprint_hex(&self) -> String {
        use std::fmt::Write as _;
        self.verify
            .thumbprint()
            .iter()
            .fold(String::new(), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            })
    }
}

/// What a call came back as.
#[derive(Debug)]
pub struct Reply {
    /// The HTTP status.
    pub status: u16,
    /// `true` when the answer was a COSE message that verified against the
    /// pinned server key and was opened; `false` for an unsigned answer (the
    /// layer's own refusals), whose body is then the raw bytes.
    pub sealed: bool,
    /// The response headers.
    pub headers: HeaderMap,
    /// The opened payload (or, unsigned, the raw body).
    pub body: Vec<u8>,
}

impl Reply {
    /// The body as JSON.
    ///
    /// # Panics
    ///
    /// When the body is not JSON.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "status {} body is not JSON ({error}): {:?}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The body as text, for assertion messages.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A client that seals every call with one key and verifies every answer
/// against the server keys it pins.
#[derive(Clone)]
pub struct SealedClient {
    base_url: String,
    envelope: ClientEnvelope,
    contracts: OpContracts,
    http: reqwest::Client,
}

impl SealedClient {
    /// A client of `base_url` (`http://127.0.0.1:port`, no trailing slash)
    /// signing with `key` and trusting answers signed by any of
    /// `server_public_keys` (the server's `current` and `next`).
    ///
    /// # Panics
    ///
    /// When the envelope refuses its configuration, which a suite cannot
    /// recover from.
    #[must_use]
    pub fn new(
        base_url: &str,
        key: &TestKey,
        server_public_keys: &[[u8; 32]],
        contracts: OpContracts,
    ) -> Self {
        crate::install_default_crypto_provider();
        let mut resolver = StaticVerifierResolver::new();
        for public in server_public_keys {
            resolver = resolver.with_key(
                CoseVerifyKey::ed25519(public).expect("a server public key is an Ed25519 point"),
            );
        }
        let cose = CoseEnvelope::client(CoseMode::Sign1, key.signer.clone(), Arc::new(resolver))
            .build()
            .expect("a client envelope with an Ed25519 or P-256 signer builds");
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            envelope: ClientEnvelope::new(cose, AUDIENCE).expect("a non-empty audience"),
            contracts,
            http: reqwest::Client::new(),
        }
    }

    /// Starts a call: `method` and the route `template` as the schema writes it
    /// (`/messages/{id}`, `/$procs/sendMessage`).
    #[must_use]
    pub fn request<'a>(&'a self, method: &'a str, template: &'a str) -> CallBuilder<'a> {
        CallBuilder {
            client: self,
            method,
            template,
            params: Vec::new(),
            query: None,
            body: None,
            payload_type: PAYLOAD_TYPE.to_owned(),
            idempotency_key: None,
            if_match: None,
            extra_headers: Vec::new(),
        }
    }
}

/// One call under construction.
pub struct CallBuilder<'a> {
    client: &'a SealedClient,
    method: &'a str,
    template: &'a str,
    params: Vec<String>,
    query: Option<String>,
    body: Option<Vec<u8>>,
    payload_type: String,
    idempotency_key: Option<String>,
    if_match: Option<String>,
    extra_headers: Vec<(String, String)>,
}

/// A call sealed but not yet sent, so a suite can deliver the same bytes twice
/// (a replay) or tamper with the headers.
pub struct Prepared {
    url: String,
    method: String,
    sealed: SealedCall,
    extra_headers: Vec<(String, String)>,
    http: reqwest::Client,
}

impl CallBuilder<'_> {
    /// The values of the template's `{parameters}`, in order.
    #[must_use]
    pub fn params(mut self, params: &[&str]) -> Self {
        self.params = params.iter().map(|param| (*param).to_owned()).collect();
        self
    }

    /// The query string as sent (`limit=5`).
    #[must_use]
    pub fn query(mut self, query: &str) -> Self {
        self.query = Some(query.to_owned());
        self
    }

    /// A JSON request payload.
    ///
    /// # Panics
    ///
    /// When `value` does not serialise, which a `serde_json::Value` always does.
    #[must_use]
    pub fn json(mut self, value: &serde_json::Value) -> Self {
        self.body = Some(serde_json::to_vec(value).expect("a JSON value serialises"));
        self
    }

    /// A raw request payload of an explicit type, for a body no JSON value can
    /// express (undecodable text, a CBOR document). The type must be a
    /// sealable `type/subtype`; the server accepts only `application/json`, so
    /// any other is what a test sends to see it refused.
    #[must_use]
    pub fn payload(mut self, bytes: &[u8], payload_type: &str) -> Self {
        self.body = Some(bytes.to_vec());
        payload_type.clone_into(&mut self.payload_type);
        self
    }

    /// The `Idempotency-Key`, which the seal binds.
    #[must_use]
    pub fn idempotency_key(mut self, key: &str) -> Self {
        self.idempotency_key = Some(key.to_owned());
        self
    }

    /// The `If-Match`, which the seal binds.
    #[must_use]
    pub fn if_match(mut self, value: &str) -> Self {
        self.if_match = Some(value.to_owned());
        self
    }

    /// A header the seal does not bind, sent as given (an `Authorization`
    /// header, to prove the server refuses it).
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.extra_headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Seals the call without sending it.
    ///
    /// # Panics
    ///
    /// When the route is not in the contract table or the signer fails.
    pub async fn seal(self) -> Prepared {
        let client = self.client;
        let contract = *cratestack::find_contract(client.contracts, self.method, self.template)
            .unwrap_or_else(|| {
                panic!(
                    "no contract digest for `{} {}`: the route is not in the schema",
                    self.method, self.template
                )
            });
        let params: Vec<&str> = self.params.iter().map(String::as_str).collect();
        let mut path = String::new();
        let mut next = params.iter();
        for segment in self.template.split('/') {
            if segment.is_empty() {
                continue;
            }
            path.push('/');
            if segment.starts_with('{') && segment.ends_with('}') {
                path.push_str(&encode_segment(
                    next.next().expect("a value per {parameter}"),
                ));
            } else {
                path.push_str(segment);
            }
        }
        let mut call = SealCall::new(self.method, RouteRef::new(self.template, &params), contract)
            .payload(self.body.as_deref().unwrap_or_default(), &self.payload_type)
            .accept(PAYLOAD_TYPE)
            .query(self.query.as_deref());
        if let Some(key) = self.idempotency_key.as_deref() {
            call = call.idempotency_key(key);
        }
        if let Some(value) = self.if_match.as_deref() {
            call = call.if_match(value);
        }
        let sealed = client
            .envelope
            .seal_call(call)
            .await
            .expect("sealing a call with a test key succeeds");
        let mut url = format!("{}{path}", client.base_url);
        if let Some(query) = &self.query {
            url.push('?');
            url.push_str(query);
        }
        Prepared {
            url,
            method: self.method.to_owned(),
            sealed,
            extra_headers: self.extra_headers,
            http: client.http.clone(),
        }
    }

    /// Seals, sends and opens the answer.
    ///
    /// # Panics
    ///
    /// On a transport failure, or an answer that claims to be sealed but does
    /// not verify against the pinned server keys: a suite must never read a
    /// body it could not authenticate.
    pub async fn send(self) -> Reply {
        self.seal().await.send().await
    }
}

fn encode_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

impl Prepared {
    /// Delivers the call once and returns the raw answer.
    async fn deliver(&self) -> (u16, HeaderMap, bytes::Bytes) {
        let mut request = self
            .http
            .request(
                reqwest::Method::from_bytes(self.method.as_bytes()).expect("a valid method"),
                &self.url,
            )
            .body(self.sealed.body.clone());
        for (name, value) in &self.sealed.headers {
            request = request.header(name.clone(), value.clone());
        }
        for (name, value) in &self.extra_headers {
            request = request.header(
                HeaderName::from_bytes(name.as_bytes()).expect("a valid header name"),
                HeaderValue::from_str(value).expect("a valid header value"),
            );
        }
        let response = request.send().await.expect("the gateway answers");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().await.expect("the answer has a body");
        (status, headers, body)
    }

    /// Sends the call and opens the answer. Consumes `self`: the answer can
    /// be opened only once, against this call's own digest.
    ///
    /// # Panics
    ///
    /// See [`CallBuilder::send`].
    pub async fn send(self) -> Reply {
        let (status, headers, body) = self.deliver().await;
        let is_cose = headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/cose"));
        if !is_cose {
            return Reply {
                status,
                sealed: false,
                headers,
                body: body.to_vec(),
            };
        }
        let opened = self
            .sealed
            .pending
            .open(status, &headers, body)
            .await
            .unwrap_or_else(|error| panic!("a sealed answer failed to verify: {error}"));
        Reply {
            status,
            sealed: true,
            headers,
            body: opened.body.to_vec(),
        }
    }

    /// Delivers the same sealed bytes and returns only the status and whether
    /// the answer was sealed, never opening it: for the second delivery of a
    /// replay, which the layer refuses with an unsigned 401.
    pub async fn resend(&self) -> Reply {
        let (status, headers, body) = self.deliver().await;
        let sealed = headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/cose"));
        Reply {
            status,
            sealed,
            headers,
            body: body.to_vec(),
        }
    }
}
