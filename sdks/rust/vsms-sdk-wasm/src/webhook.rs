//! Opening a sealed webhook (VS-8), free of `wasm-bindgen` so a host
//! `cargo test` runs it.
//!
//! vsms delivers an event as a sealed request signed with the gateway's own
//! envelope signing key. [`WebhookOpener`] is `vsms-sdk-rust`'s
//! `WebhookOpener` for the Node SDK: the same binding (audience, `POST`, the
//! route, the delivery contract, `application/json`), the same pinned keys,
//! the same nonce rule, and the same check of the endpoint id and the app id
//! inside the verified envelope, so the two cannot disagree about a byte. It returns the verified envelope as JSON text; the TypeScript side
//! types it.
//!
//! The constants below are copies of `sms_webhook`'s and `vsms-sdk-rust`'s, held
//! equal by `cargo xtask sdk-parity-check`: the SDK crates stay free of the
//! gateway's.

use std::borrow::Cow;
use std::sync::Arc;

use bytes::Bytes;
use cratestack::cose::{
    CoseEnvelope, CoseMode, CoseVerifyKey, Ed25519Signer, StaticVerifierResolver,
};
use cratestack::{
    serde_json, Binding, BoundHeaders, CratestackError, InMemoryNonceStore, NonceStore, PathParams,
};
use sha2::{Digest, Sha256};

use crate::core::ServerKeys;

/// The method every delivery uses.
const METHOD: &str = "POST";

/// The one payload type a delivery is sealed under.
const PAYLOAD_TYPE: &str = "application/json";

/// Domain-separation tag of [`webhook_contract_sha`].
pub const WEBHOOK_CONTRACT_DOMAIN: &[u8] = b"vsms/webhook-delivery/v2\0";

/// The declared shape of a delivery, hashed into the contract digest.
pub const WEBHOOK_CONTRACT_DOCUMENT: &str = r#"{"contract":"vsms.webhook.delivery","version":2,"method":"POST","request":{"payload_type":"application/json","body":{"id":"string","eventId":"uuid","type":"string","occurredAt":"rfc3339-utc-seconds","appId":"string","endpointId":"string","data":"object"}},"response":{"payload_type":"application/json","body":"not read"}}"#;

/// The digest a delivery binds.
#[must_use]
pub fn webhook_contract_sha() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(WEBHOOK_CONTRACT_DOMAIN);
    hasher.update(WEBHOOK_CONTRACT_DOCUMENT.as_bytes());
    hasher.finalize().into()
}

/// Why a delivery was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookError {
    /// Not a message that verifies under a pinned server key for this audience,
    /// route and contract, or one that does and was made for another app or
    /// endpoint. One answer for every reason, on purpose.
    Unverified,
    /// It verified, and the payload is not UTF-8 text.
    Unreadable,
    /// It verified, and the payload is not an envelope with a string `appId`
    /// and a string `endpointId`, each named once: a bug on one side, reported
    /// apart from a refusal.
    Malformed(String),
    /// The nonce store or the clock failed.
    Backend(String),
    /// The opener could not be built.
    Invalid(String),
}

impl std::fmt::Display for WebhookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unverified => f.write_str(
                "the delivery does not verify under a pinned server key for this receiver",
            ),
            Self::Unreadable => f.write_str("the delivery verified but its payload is not UTF-8"),
            Self::Malformed(message) => write!(
                f,
                "the delivery verified but is not a webhook envelope: {message}"
            ),
            Self::Backend(message) | Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for WebhookError {}

/// Opens deliveries addressed to one receiver.
pub struct WebhookOpener {
    cose: CoseEnvelope,
    audience: String,
    route: String,
    app_id: String,
    endpoint_id: String,
}

/// The two fields the opener compares. A duplicate of either is an error, as in
/// `vsms-sdk-rust`'s typed event, and not "the last one wins" as it is in a
/// `serde_json::Value`: the gateway builds the envelope from a map and can never
/// emit one, so a body that has one is not the documented envelope.
#[derive(serde::Deserialize)]
struct Ids {
    #[serde(rename = "appId")]
    app_id: String,
    #[serde(rename = "endpointId")]
    endpoint_id: String,
}

impl std::fmt::Debug for WebhookOpener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookOpener")
            .field("audience", &self.audience)
            .field("route", &self.route)
            .field("app_id", &self.app_id)
            .field("endpoint_id", &self.endpoint_id)
            .finish_non_exhaustive()
    }
}

impl WebhookOpener {
    /// An opener for deliveries to `route` that name `audience` and were made
    /// for the app `app_id` through the endpoint `endpoint_id`, believing `server`'s two keys, with an in-memory nonce store
    /// and the default skew. `clock`, when given, replaces the system clock
    /// (Unix seconds): for tests and for replaying a recorded delivery.
    ///
    /// Both ids are required on purpose: one gateway key signs every app's
    /// deliveries, so a seal alone says only that it reached this audience.
    /// The endpoint id is the binding (the one signed value no one but the
    /// gateway chooses); the app id additionally catches an endpoint registered
    /// under another app.
    ///
    /// # Errors
    ///
    /// [`WebhookError::Invalid`] for an empty audience, app id or endpoint id,
    /// a route that does not start with `/`, or a key the envelope refuses.
    pub fn new(
        server: &ServerKeys,
        audience: &str,
        route: &str,
        app_id: &str,
        endpoint_id: &str,
        clock: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
    ) -> Result<Self, WebhookError> {
        Self::with_nonce_store(
            server,
            audience,
            route,
            app_id,
            endpoint_id,
            clock,
            Arc::new(InMemoryNonceStore::new()),
        )
    }

    /// [`WebhookOpener::new`] with the nonce store the caller supplies.
    ///
    /// # Errors
    ///
    /// As [`WebhookOpener::new`].
    pub fn with_nonce_store(
        server: &ServerKeys,
        audience: &str,
        route: &str,
        app_id: &str,
        endpoint_id: &str,
        clock: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
        nonces: Arc<dyn NonceStore>,
    ) -> Result<Self, WebhookError> {
        if audience.is_empty() {
            return Err(WebhookError::Invalid(
                "a webhook opener needs the audience the endpoint was created with".to_owned(),
            ));
        }
        if !route.starts_with('/') {
            return Err(WebhookError::Invalid(
                "a webhook route is the path of the endpoint's URL, such as /webhooks/vsms"
                    .to_owned(),
            ));
        }
        if app_id.is_empty() {
            return Err(WebhookError::Invalid(
                "a webhook opener needs the id of the app the endpoint belongs to: the audience \
                 alone does not say whose delivery it is"
                    .to_owned(),
            ));
        }
        if endpoint_id.is_empty() {
            return Err(WebhookError::Invalid(
                "a webhook opener needs the id of the endpoint it serves: the audience and the \
                 app id do not say which endpoint of the app a delivery was made through"
                    .to_owned(),
            ));
        }
        let invalid = |_| WebhookError::Invalid("a pinned server key is invalid".to_owned());
        let mut resolver = StaticVerifierResolver::new();
        for key in [&server.current, &server.next] {
            resolver = resolver.with_key(CoseVerifyKey::ed25519(key).map_err(invalid)?);
        }
        // A server envelope insists on a signer because it also seals answers.
        // An opener never does, so any key will do. The seed is a constant on
        // purpose: this key never signs and nobody pins it, so there is nothing
        // to protect and no randomness to ask the host for.
        let signer = Ed25519Signer::from_seed(&[0x5a; 32]);
        #[allow(
            clippy::arc_with_non_send_sync,
            reason = "on wasm32 cratestack's MaybeSend is empty and the module is single-threaded, \
                      so the Arc never crosses a thread; on a host the signer is Send + Sync and the \
                      lint does not fire, which is why this is an `allow` and not an `expect`"
        )]
        let signer = Arc::new(signer);
        let mut builder = CoseEnvelope::server(CoseMode::Sign1, signer, Arc::new(resolver), nonces);
        if let Some(clock) = clock {
            builder = builder.clock(move || clock());
        }
        let cose = builder
            .build()
            .map_err(|error| WebhookError::Invalid(error.to_string()))?;
        Ok(Self {
            cose,
            audience: audience.to_owned(),
            route: route.to_owned(),
            app_id: app_id.to_owned(),
            endpoint_id: endpoint_id.to_owned(),
        })
    }

    /// Opens one delivery's body (the raw request bytes) and returns the
    /// verified envelope as JSON text.
    ///
    /// # Errors
    ///
    /// [`WebhookError::Unverified`] for anything that does not verify and for a
    /// verified delivery made for another app or endpoint,
    /// [`WebhookError::Unreadable`] for a verified payload that is not UTF-8,
    /// [`WebhookError::Malformed`] for one that is not an envelope naming its
    /// app and endpoint once each, [`WebhookError::Backend`] when the nonce
    /// store fails.
    pub async fn open(&self, body: &[u8]) -> Result<String, WebhookError> {
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
        let text =
            String::from_utf8(opened.payload.to_vec()).map_err(|_| WebhookError::Unreadable)?;
        let ids: Ids = serde_json::from_str(&text)
            .map_err(|error| WebhookError::Malformed(error.to_string()))?;
        if ids.endpoint_id != self.endpoint_id {
            return Err(WebhookError::Unverified);
        }
        if ids.app_id != self.app_id {
            return Err(WebhookError::Unverified);
        }
        Ok(text)
    }
}
