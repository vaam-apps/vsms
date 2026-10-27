//! The sealing path, free of `wasm-bindgen` so a host `cargo test` runs it.
//!
//! [`Sealer`] is what `vsms-sdk-rust`'s `VsmsClient::send_message` does up to
//! the HTTP request and from the HTTP answer on, with the transport left to the
//! caller.

use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use cratestack::client_rust::{
    encode_path_segment, ClientEnvelope, ClientError, EnvelopeError, PendingResponse, RouteRef,
    SealCall,
};
use cratestack::cose::{
    CoseEnvelope, CoseMode, CoseSigner, CoseVerifyKey, Ed25519Signer, StaticVerifierResolver,
};
use http::{HeaderMap, HeaderName, HeaderValue};
use zeroize::Zeroize as _;

use crate::contracts::contract_digest;

/// The audience every request binds: the gateway's configured name, never its
/// host (cratestack ADR 0006 section 4). A constant of the protocol, the same
/// one `vsms-sdk-rust` and `sms_api::envelope` carry; `cargo xtask
/// sdk-parity-check` holds the three to each other.
pub const AUDIENCE: &str = "vsms";

/// The only payload type the gateway seals, in both directions.
const PAYLOAD_TYPE: &str = "application/json";

/// Why a call could not be sealed. Nothing was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealError {
    /// The method and route address no operation of the schema, or the
    /// parameters do not fit the route. The message names them.
    Unbound(String),
    /// A key is not usable, or the signer failed, or the call could not be
    /// encoded. The message is the envelope's own and never quotes a key.
    Seal(String),
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unbound(message) | Self::Seal(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SealError {}

/// Why an answer was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The answer is not a sealed message: a proxy's page, or the gateway's
    /// coarse `401` (which is never sealed). Nothing in its body was read.
    Unsigned {
        /// The status it came with.
        status: u16,
    },
    /// The answer claims to be sealed and does not verify under either pinned
    /// server key, or under this request.
    Unverified,
    /// Anything else, including a header that could not be read.
    Other(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsigned { status } => write!(
                f,
                "the answer ({status}) is not sealed under a pinned server key"
            ),
            Self::Unverified => f.write_str("the answer does not verify under a pinned server key"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for OpenError {}

/// The gateway's pinned response keys, the pair `sms-gateway server-keys`
/// prints. Both are required and distinct, exactly as `vsms-sdk-rust`'s
/// `ServerKeys` holds them: with one pinned, a rotation is a flag day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerKeys {
    current: [u8; 32],
    next: [u8; 32],
}

impl ServerKeys {
    /// # Errors
    ///
    /// [`SealError::Seal`] when a key is not an Ed25519 point or when both are
    /// the same key.
    pub fn new(current: [u8; 32], next: [u8; 32]) -> Result<Self, SealError> {
        for (label, key) in [("current", &current), ("next", &next)] {
            CoseVerifyKey::ed25519(key).map_err(|_| {
                SealError::Seal(format!("the {label} server key is not an Ed25519 key"))
            })?;
        }
        if current == next {
            return Err(SealError::Seal(
                "the current and next server keys are the same key".to_owned(),
            ));
        }
        Ok(Self { current, next })
    }
}

/// Where the signature comes from.
pub enum SealerKey {
    /// An Ed25519 seed this crate signs with.
    Seed([u8; 32]),
    /// A signer that lives elsewhere (`ExternalSigner::ed25519` over a JS
    /// callback: a Node `KeyObject`, a KMS). Built by the caller;
    /// `public_key` is the registered half, 32 bytes.
    Ed25519 {
        /// The registered public key.
        public_key: [u8; 32],
        /// The signer.
        signer: Arc<dyn CoseSigner>,
    },
    /// A P-256 key behind a callback (`ExternalSigner::esp256`): the operator
    /// console's WebCrypto key, which is never extractable. `public_key` is
    /// SEC1, 65 bytes uncompressed or 33 compressed.
    Esp256 {
        /// The registered public key.
        public_key: Vec<u8>,
        /// The signer.
        signer: Arc<dyn CoseSigner>,
    },
}

impl std::fmt::Debug for SealerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the seed.
        match self {
            Self::Seed(_) => f.write_str("SealerKey::Seed([redacted])"),
            Self::Ed25519 { public_key, .. } => f
                .debug_struct("SealerKey::Ed25519")
                .field("public_key", &URL_SAFE_NO_PAD.encode(public_key))
                .finish_non_exhaustive(),
            Self::Esp256 { public_key, .. } => f
                .debug_struct("SealerKey::Esp256")
                .field("public_key", &URL_SAFE_NO_PAD.encode(public_key))
                .finish_non_exhaustive(),
        }
    }
}

/// Which algorithm a [`Sealer`] signs under. `ClientKey` rows are Ed25519 and
/// `ConsoleKey` rows ESP256, and the gateway reads each table only for its own
/// algorithm, so the choice is the caller's identity and not a preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    /// COSE `-19`: an `AppClient`, the Node SDK's identity.
    Ed25519,
    /// COSE `-9`: a human console's paired key.
    Esp256,
}

impl Algorithm {
    /// The name the JavaScript side reads.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ed25519 => "ed25519",
            Self::Esp256 => "esp256",
        }
    }
}

/// A caller's sealing identity: the key it signs with and the server keys it
/// trusts. The audience is [`AUDIENCE`].
#[derive(Debug)]
pub struct Sealer {
    envelope: ClientEnvelope,
    public_key: Vec<u8>,
    algorithm: Algorithm,
}

impl Sealer {
    /// Builds the sealer.
    ///
    /// # Errors
    ///
    /// [`SealError::Seal`] if a key is not a point or the envelope cannot be
    /// built.
    pub fn new(key: SealerKey, server: &ServerKeys) -> Result<Self, SealError> {
        let (signer, public_key, algorithm): (Arc<dyn CoseSigner>, Vec<u8>, Algorithm) = match key {
            SealerKey::Seed(mut seed) => {
                let signer = Ed25519Signer::from_seed(&seed);
                // The signer holds its own copy; this one is not needed again.
                seed.zeroize();
                let public_key = signer
                    .verify_key()
                    .ed25519_bytes()
                    .ok_or_else(|| SealError::Seal("the seed has no Ed25519 public key".into()))?;
                (Arc::new(signer), public_key.to_vec(), Algorithm::Ed25519)
            }
            SealerKey::Ed25519 { public_key, signer } => {
                (signer, public_key.to_vec(), Algorithm::Ed25519)
            }
            SealerKey::Esp256 { public_key, signer } => {
                let canonical = CoseVerifyKey::p256_sec1(&public_key)
                    .ok()
                    .and_then(|key| key.p256_sec1_uncompressed())
                    .ok_or_else(|| SealError::Seal("the public key is not a P-256 key".into()))?;
                (signer, canonical.to_vec(), Algorithm::Esp256)
            }
        };
        let mut resolver = StaticVerifierResolver::new();
        for key in [&server.current, &server.next] {
            resolver = resolver.with_key(
                CoseVerifyKey::ed25519(key)
                    .map_err(|_| SealError::Seal("a pinned server key is invalid".into()))?,
            );
        }
        let cose = CoseEnvelope::client(CoseMode::Sign1, signer, Arc::new(resolver))
            .build()
            .map_err(|error| SealError::Seal(error.to_string()))?;
        let envelope = ClientEnvelope::new(cose, AUDIENCE)
            .map_err(|error| SealError::Seal(error.to_string()))?;
        Ok(Self {
            envelope,
            public_key,
            algorithm,
        })
    }

    /// The registered public half: 32 bytes for Ed25519, 65 (SEC1
    /// uncompressed, the canonical form `registerConsoleKey` takes) for ESP256.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// The algorithm this sealer signs under.
    #[must_use]
    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }

    /// Seals one call.
    ///
    /// `template` is the schema's route (`/messages/{id}`), `params` the
    /// **decoded** values of its `{parameters}` in order, `body` the JSON text
    /// of a write, `query` the query string as it will be sent (without the
    /// `?`) and `idempotency_key` the `Idempotency-Key` the request is sent
    /// with. The operation, and with it the contract digest the signature
    /// binds, is found from the schema; a method and route it does not serve
    /// are refused before anything is signed. The target to send to is in
    /// [`Sealed::target`], built from the very parameters that were bound.
    ///
    /// # Errors
    ///
    /// [`SealError::Unbound`] or [`SealError::Seal`].
    pub async fn seal_call(
        &self,
        method: &str,
        template: &str,
        params: &[&str],
        body: Option<&str>,
        query: Option<&str>,
        idempotency_key: Option<&str>,
    ) -> Result<Sealed, SealError> {
        self.seal(
            method,
            template,
            params,
            body,
            query,
            &Bound {
                idempotency_key,
                if_match: None,
            },
        )
        .await
    }

    /// [`Sealer::seal_call`] with every header a signature can bind: a write
    /// under optimistic locking sends `If-Match`, and the gateway rebuilds the
    /// binding from the header it receives, so the client must have bound that
    /// same value.
    ///
    /// # Errors
    ///
    /// [`SealError::Unbound`] or [`SealError::Seal`].
    pub async fn seal(
        &self,
        method: &str,
        template: &str,
        params: &[&str],
        body: Option<&str>,
        query: Option<&str>,
        bound: &Bound<'_>,
    ) -> Result<Sealed, SealError> {
        let digest = contract_digest(method, template)?;
        let path = concrete_path(template, params)?;
        let query = query.filter(|query| !query.is_empty());
        let mut call = SealCall::new(method, RouteRef::new(template, params), digest)
            .query(query)
            .payload(body.unwrap_or("").as_bytes(), PAYLOAD_TYPE)
            .accept(PAYLOAD_TYPE);
        if let Some(key) = bound.idempotency_key {
            call = call.idempotency_key(key);
        }
        if let Some(if_match) = bound.if_match {
            call = call.if_match(if_match);
        }
        let sealed = self
            .envelope
            .seal_call(call)
            .await
            .map_err(|error| SealError::Seal(error.to_string()))?;
        Ok(Sealed {
            body: sealed.body.to_vec(),
            headers: sealed
                .headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_owned(),
                        String::from_utf8_lossy(value.as_bytes()).into_owned(),
                    )
                })
                .collect(),
            target: match query {
                Some(query) => format!("{path}?{query}"),
                None => path,
            },
            pending: Some(sealed.pending),
        })
    }
}

/// The request headers a signature binds besides the payload type: what the
/// caller sends as `Idempotency-Key` and `If-Match`, bound as sent.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bound<'a> {
    /// The write's `Idempotency-Key`.
    pub idempotency_key: Option<&'a str>,
    /// The `If-Match` of a write under optimistic locking, a strong ETag
    /// (`"3"`, quotes included).
    pub if_match: Option<&'a str>,
}

/// `template` with each `{parameter}` replaced by the matching entry of
/// `params`, percent-encoded as one path segment. The count must match: a
/// route sealed with the wrong number of parameters would bind something the
/// URL does not say, and a parameter must survive a URL parser as the segment
/// it is (so `.` and `..` are refused).
pub(crate) fn concrete_path(template: &str, params: &[&str]) -> Result<String, SealError> {
    let mut remaining = params.iter();
    let mut path = String::with_capacity(template.len());
    for (index, segment) in template.split('/').enumerate() {
        if index > 0 {
            path.push('/');
        }
        if segment.starts_with('{') && segment.ends_with('}') {
            let value = remaining.next().ok_or_else(|| {
                SealError::Unbound(format!(
                    "the route `{template}` takes more parameters than the {} given",
                    params.len()
                ))
            })?;
            if value.is_empty() {
                return Err(SealError::Unbound(format!(
                    "a parameter of `{template}` is empty"
                )));
            }
            // `.` and `..` survive percent-encoding (they are unreserved) and
            // are then removed by any URL parser, so the request would go to a
            // parent path while the signature binds the template with the
            // parameter in it. Refused here, where the binding is made.
            if matches!(*value, "." | "..") {
                return Err(SealError::Unbound(format!(
                    "a parameter of `{template}` is a dot segment, which a URL parser would \
                     resolve away"
                )));
            }
            path.push_str(&encode_path_segment(value));
        } else {
            path.push_str(segment);
        }
    }
    if remaining.next().is_some() {
        return Err(SealError::Unbound(format!(
            "the route `{template}` takes fewer parameters than the {} given",
            params.len()
        )));
    }
    Ok(path)
}

/// A sealed call: the bytes and headers to send as given, and what the answer
/// is opened with.
#[derive(Debug)]
pub struct Sealed {
    /// The request body: the COSE message.
    pub body: Vec<u8>,
    /// Every header the seal depends on (`Content-Type`, `Accept`,
    /// `Cratestack-Contract`, the payload-type selectors, the bound
    /// `Idempotency-Key`). Anything else is the caller's to add.
    pub headers: Vec<(String, String)>,
    /// Where to send it, relative to the gateway's origin: the path with its
    /// parameters in place, then `?query` if there was one.
    pub target: String,
    pending: Option<PendingResponse>,
}

impl Sealed {
    /// Verifies the answer against the request this was made for and returns
    /// its payload (the JSON text). Spent by the call: a second one is an
    /// [`OpenError::Other`].
    ///
    /// # Errors
    ///
    /// See [`OpenError`].
    pub async fn open_response(
        &mut self,
        status: u16,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<Vec<u8>, OpenError> {
        let pending = self
            .pending
            .take()
            .ok_or_else(|| OpenError::Other("this call's answer was already opened".into()))?;
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| OpenError::Other("a response header name is not valid".into()))?;
            let value = HeaderValue::from_str(value)
                .map_err(|_| OpenError::Other("a response header value is not valid".into()))?;
            map.append(name, value);
        }
        match pending.open(status, &map, body.into()).await {
            Ok(opened) => Ok(opened.body.to_vec()),
            Err(ClientError::Envelope(EnvelopeError::Unsigned { status })) => {
                Err(OpenError::Unsigned { status })
            }
            Err(ClientError::Envelope(EnvelopeError::Unverified)) => Err(OpenError::Unverified),
            Err(other) => Err(OpenError::Other(other.to_string())),
        }
    }
}
