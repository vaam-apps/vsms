//! Opening a sealed webhook (VS-8).
//!
//! vsms delivers an event as a **sealed request**: a `COSE_Sign1` envelope
//! signed with the gateway's own envelope signing key, the same key that signs
//! its answers. There is no shared secret and no signature header. A receiver
//! pins the two public keys it already pins to open vsms's answers
//! ([`ServerKeys`]) and opens the body:
//!
//! ```
//! # async fn demo(body: &[u8]) -> Result<(), vsms_sdk::WebhookError> {
//! use vsms_sdk::{ServerKeys, WebhookOpener};
//!
//! let keys = ServerKeys::parse_report(
//!     "current: 11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo\n\
//!      next: PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw\n",
//! )
//! .unwrap();
//! // `audience` is the name this receiver chose when its endpoint was created;
//! // `route` is the path of the endpoint's URL.
//! let opener = WebhookOpener::new(&keys, "vaam", "/webhooks/vsms").unwrap();
//! let event = opener.open(body).await?;
//! println!("{} {}", event.event_type, event.event_id);
//! # Ok(())
//! # }
//! ```
//!
//! What is checked, in one coarse answer ([`WebhookError::Unverified`]) that
//! never says which: the signature under a pinned key, the audience, the route,
//! the method, the delivery contract ([`WEBHOOK_CONTRACT_SHA`]), the payload
//! type, the timestamp (within the skew, 300 s by default) and the nonce, which
//! is recorded so a replayed body is refused.
//!
//! **The nonce store is per process by default.** A receiver with several
//! replicas must pass one they share ([`WebhookOpener::with_nonce_store`]),
//! or a body replayed to another replica inside the skew window opens twice.
//! The event's `id` is the application-level dedupe key either way: delivery is
//! at-least-once, and a retry carries the same `id` in a new sealed request.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cratestack::cose::{CoseEnvelope, CoseMode, CoseVerifyKey, StaticVerifierResolver};
use cratestack::{
    Binding, BoundHeaders, CratestackError, InMemoryNonceStore, NonceStore, PathParams,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::SdkError;
use crate::server_keys::ServerKeys;

/// The method every delivery uses.
const METHOD: &str = "POST";

/// The one payload type a delivery is sealed under.
const PAYLOAD_TYPE: &str = "application/json";

/// Domain-separation tag of [`webhook_contract_sha`]. The same bytes as
/// `sms_webhook::CONTRACT_DOMAIN` in the gateway; `cargo xtask sdk-parity-check`
/// holds them equal.
pub const WEBHOOK_CONTRACT_DOMAIN: &[u8] = b"vsms/webhook-delivery/v1\0";

/// The declared shape of a delivery, hashed into [`WEBHOOK_CONTRACT_SHA`]. The
/// same bytes as `sms_webhook::CONTRACT_DOCUMENT`; `cargo xtask
/// sdk-parity-check` holds them equal.
pub const WEBHOOK_CONTRACT_DOCUMENT: &str = r#"{"contract":"vsms.webhook.delivery","version":1,"method":"POST","request":{"payload_type":"application/json","body":{"id":"string","eventId":"uuid","type":"string","occurredAt":"rfc3339-utc-seconds","data":"object"}},"response":{"payload_type":"application/json","body":"not read"}}"#;

/// The digest a delivery binds, as lowercase hex:
/// `SHA-256(WEBHOOK_CONTRACT_DOMAIN || WEBHOOK_CONTRACT_DOCUMENT)`. A receiver
/// that runs its own cratestack `EnvelopeLayer` declares it for the delivery
/// route; this crate computes it ([`webhook_contract_sha`]) and its tests pin
/// this literal.
pub const WEBHOOK_CONTRACT_SHA_HEX: &str =
    "7c5a6ab84270ec64eeb668cf50a031ac436433a08467cefe142e338c93132b62";

/// [`WEBHOOK_CONTRACT_SHA_HEX`] as bytes, computed from the document.
///
/// ```
/// let sha = vsms_sdk::webhook_contract_sha();
/// let hex: String = sha.iter().map(|byte| format!("{byte:02x}")).collect();
/// assert_eq!(hex, vsms_sdk::WEBHOOK_CONTRACT_SHA_HEX);
/// ```
#[must_use]
pub fn webhook_contract_sha() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(WEBHOOK_CONTRACT_DOMAIN);
    hasher.update(WEBHOOK_CONTRACT_DOCUMENT.as_bytes());
    hasher.finalize().into()
}

/// Why a delivery was not accepted. Nothing in it was read.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebhookError {
    /// The body is not a message that verifies under a pinned server key for
    /// this audience, route and contract: a forgery, another receiver's
    /// delivery, a body sealed by a key you did not pin, a stale timestamp
    /// (check this machine's clock), a replay, or a flipped bit. It is one
    /// answer for all of them on purpose; answer the request with a `401`.
    #[error("the delivery does not verify under a pinned server key for this receiver")]
    Unverified,
    /// The body verified and is not the envelope vsms documents. This is a bug
    /// on one side: report it, and answer `2xx` only if you mean to drop the
    /// event.
    #[error("the delivery verified but is not a webhook envelope: {0}")]
    Malformed(String),
    /// The nonce store or the clock failed. Answer `5xx`; vsms will retry.
    #[error("the delivery could not be checked: {0}")]
    Backend(String),
}

/// A webhook event, opened.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WebhookEvent {
    /// `WebhookAttempt.id`: the dedupe key. A retry and an operator's replay
    /// carry the same one.
    pub id: String,
    /// The source event's id (`WebhookAttempt.sourceEventId`): what the
    /// `X-Sms-Event-Id` header carried before webhooks were sealed.
    #[serde(rename = "eventId")]
    pub event_id: String,
    /// `message.delivered` and the rest of the catalogue in
    /// `docs/architecture.md` section 8.4.
    #[serde(rename = "type")]
    pub event_type: String,
    /// When the delivery was made, RFC 3339 in UTC: a documented approximation
    /// of when the event happened (section 8.5).
    #[serde(rename = "occurredAt")]
    pub occurred_at: String,
    /// The event's payload, exactly as vsms sent it.
    pub data: serde_json::Value,
}

/// Opens deliveries addressed to one receiver.
pub struct WebhookOpener {
    cose: CoseEnvelope,
    audience: String,
    route: String,
}

impl std::fmt::Debug for WebhookOpener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookOpener")
            .field("audience", &self.audience)
            .field("route", &self.route)
            .finish_non_exhaustive()
    }
}

/// Builds a [`WebhookOpener`] with a shared nonce store, another skew or a
/// pinned clock.
pub struct WebhookOpenerBuilder<'a> {
    keys: &'a ServerKeys,
    audience: String,
    route: String,
    nonces: Option<Arc<dyn NonceStore>>,
    skew: Option<Duration>,
    clock: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
}

impl WebhookOpener {
    /// An opener for deliveries to the route `route` (the path of the
    /// endpoint's URL, such as `/webhooks/vsms`) that name `audience`, with an
    /// in-memory nonce store and the default 300 s skew.
    ///
    /// # Errors
    ///
    /// [`SdkError::Config`] for an empty audience, a route that does not start
    /// with `/`, or a pinned key the envelope refuses.
    pub fn new(
        keys: &ServerKeys,
        audience: impl Into<String>,
        route: impl Into<String>,
    ) -> Result<Self, SdkError> {
        Self::builder(keys, audience, route).build()
    }

    /// Starts a [`WebhookOpenerBuilder`].
    pub fn builder(
        keys: &ServerKeys,
        audience: impl Into<String>,
        route: impl Into<String>,
    ) -> WebhookOpenerBuilder<'_> {
        WebhookOpenerBuilder {
            keys,
            audience: audience.into(),
            route: route.into(),
            nonces: None,
            skew: None,
            clock: None,
        }
    }

    /// Opens one delivery's body (the raw request bytes, exactly as read off
    /// the socket) and returns the event.
    ///
    /// # Errors
    ///
    /// [`WebhookError::Unverified`] for anything that does not verify,
    /// [`WebhookError::Malformed`] for a verified body that is not the
    /// envelope, [`WebhookError::Backend`] when the nonce store fails.
    pub async fn open(&self, body: &[u8]) -> Result<WebhookEvent, WebhookError> {
        let binding = Binding {
            audience: Cow::Borrowed(&self.audience),
            method: Cow::Borrowed(METHOD),
            route: Cow::Borrowed(&self.route),
            path_params: PathParams::EMPTY,
            query: None,
            contract_sha: webhook_contract_sha(),
            payload_media_type: Cow::Borrowed(PAYLOAD_TYPE),
            bound_headers: BoundHeaders::NONE,
            response: None,
        };
        let opened = self
            .cose
            .open_request(Bytes::copy_from_slice(body), &binding)
            .await
            .map_err(|error| match error {
                CratestackError::Unauthorized(_) => WebhookError::Unverified,
                other => WebhookError::Backend(other.to_string()),
            })?;
        serde_json::from_slice(&opened.payload)
            .map_err(|error| WebhookError::Malformed(error.to_string()))
    }
}

impl WebhookOpenerBuilder<'_> {
    /// A nonce store shared by every replica of the receiver. The default is
    /// per process.
    #[must_use]
    pub fn with_nonce_store(mut self, store: Arc<dyn NonceStore>) -> Self {
        self.nonces = Some(store);
        self
    }

    /// How far a delivery's timestamp may be from this machine's clock, either
    /// way. Default 300 s: a retry is sealed afresh, so there is no reason to
    /// widen it.
    #[must_use]
    pub fn skew(mut self, skew: Duration) -> Self {
        self.skew = Some(skew);
        self
    }

    /// The clock timestamps are checked against, in Unix seconds. For tests and
    /// for replaying a recorded delivery.
    #[must_use]
    pub fn clock(mut self, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.clock = Some(Arc::new(clock));
        self
    }

    /// Builds the opener.
    ///
    /// # Errors
    ///
    /// [`SdkError::Config`] for an empty audience, a route that does not start
    /// with `/`, or a pinned key the envelope refuses.
    pub fn build(self) -> Result<WebhookOpener, SdkError> {
        if self.audience.is_empty() {
            return Err(SdkError::Config(
                "a webhook opener needs the audience the endpoint was created with".to_owned(),
            ));
        }
        if !self.route.starts_with('/') {
            return Err(SdkError::Config(
                "a webhook route is the path of the endpoint's URL, such as /webhooks/vsms"
                    .to_owned(),
            ));
        }
        crate::client::ensure_crypto_provider();
        let mut resolver = StaticVerifierResolver::new();
        for public in [self.keys.current(), self.keys.next()] {
            resolver = resolver.with_key(
                CoseVerifyKey::ed25519(public)
                    .map_err(|_| SdkError::Config("a pinned server key is invalid".to_owned()))?,
            );
        }
        // The envelope insists on a signer because a server envelope also seals
        // answers. An opener never does, so a key made here and thrown away is
        // all it needs.
        let signer = crate::signer::SigningKey::generate().signer();
        let store: Arc<dyn NonceStore> = self
            .nonces
            .unwrap_or_else(|| Arc::new(InMemoryNonceStore::new()));
        let mut builder =
            CoseEnvelope::server(CoseMode::Sign1, Arc::new(signer), Arc::new(resolver), store);
        if let Some(skew) = self.skew {
            builder = builder.skew(skew);
        }
        if let Some(clock) = self.clock {
            builder = builder.clock(move || clock());
        }
        let cose = builder
            .build()
            .map_err(|error| SdkError::Config(format!("the signed transport: {error}")))?;
        Ok(WebhookOpener {
            cose,
            audience: self.audience,
            route: self.route,
        })
    }
}
