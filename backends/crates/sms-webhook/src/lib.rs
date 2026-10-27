#![doc = include_str!("lib.md")]

use cratestack::client_rust::{ClientEnvelope, ClientError, RouteRef, SealCall};
use cratestack::cose::{CoseEnvelope, CoseMode, CoseRole, Ed25519Signer, StaticVerifierResolver};
use sha2::{Digest, Sha256};
use url::Url;

/// The method every delivery uses.
pub const METHOD: &str = "POST";

/// The one payload type a delivery is sealed under, and the one it asks to be
/// answered in. The envelope below is JSON, and a receiver's layer is built
/// with `payload_media_types(["application/json"], ["application/json"])`.
pub const PAYLOAD_TYPE: &str = "application/json";

/// Domain-separation tag of [`contract_sha`], so a webhook digest can never be
/// mistaken for an op-contract digest (`cratestack/op-contract/v1`).
pub const CONTRACT_DOMAIN: &[u8] = b"vsms/webhook-delivery/v1\0";

/// The declared shape of a delivery: what [`CONTRACT_SHA`] hashes. It names the
/// method, the payload type and the fields of the envelope `hooks` builds
/// (`{id, eventId, type, occurredAt, data}`), and says the answer's body is
/// not read. Add a field, or change a type, and the digest moves: that is the
/// moment the version in [`CONTRACT_DOMAIN`] moves too.
///
/// The route is not part of it: a receiver chooses its own path, and the seal
/// binds the route separately.
pub const CONTRACT_DOCUMENT: &str = r#"{"contract":"vsms.webhook.delivery","version":1,"method":"POST","request":{"payload_type":"application/json","body":{"id":"string","eventId":"uuid","type":"string","occurredAt":"rfc3339-utc-seconds","data":"object"}},"response":{"payload_type":"application/json","body":"not read"}}"#;

/// The digest every delivery binds: `SHA-256(CONTRACT_DOMAIN || CONTRACT_DOCUMENT)`.
///
/// It is a literal so a receiver can paste it into the `AcceptedContracts`
/// table of its `EnvelopeLayer`; [`contract_sha`] recomputes it from the
/// document, and the crate's tests hold the two together (the digest was first
/// computed with Python's `hashlib`, not with this crate).
pub const CONTRACT_SHA: [u8; 32] = [
    0x7c, 0x5a, 0x6a, 0xb8, 0x42, 0x70, 0xec, 0x64, 0xee, 0xb6, 0x68, 0xcf, 0x50, 0xa0, 0x31, 0xac,
    0x43, 0x64, 0x33, 0xa0, 0x84, 0x67, 0xce, 0xfe, 0x14, 0x2e, 0x33, 0x8c, 0x93, 0x13, 0x2b, 0x62,
];

/// [`CONTRACT_SHA`] as lowercase hex, the form docs and configuration carry.
pub const CONTRACT_SHA_HEX: &str =
    "7c5a6ab84270ec64eeb668cf50a031ac436433a08467cefe142e338c93132b62";

/// The digest of [`CONTRACT_DOCUMENT`] under [`CONTRACT_DOMAIN`].
///
/// ```
/// use sms_webhook::{contract_sha, CONTRACT_SHA};
///
/// assert_eq!(contract_sha(), CONTRACT_SHA);
/// ```
#[must_use]
pub fn contract_sha() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CONTRACT_DOMAIN);
    hasher.update(CONTRACT_DOCUMENT.as_bytes());
    hasher.finalize().into()
}

/// Why a delivery could not be sealed. Nothing was sent.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SealError {
    /// The endpoint's URL cannot be a sealed route: see [`delivery_route`].
    #[error("the endpoint URL cannot carry a sealed delivery: {0}")]
    Url(&'static str),
    /// The endpoint's audience is empty, so the seal would bind no recipient.
    #[error("the endpoint has no audience, so a delivery would bind no recipient")]
    NoAudience,
    /// The signer or the envelope refused. The text is cratestack's and never
    /// quotes a key.
    #[error("sealing the delivery failed: {0}")]
    Seal(String),
}

/// The route a delivery to `url` binds: its path.
///
/// A sealed request binds the route template the receiver's router matched,
/// not the raw URL (a gateway may rewrite the host or the scheme). A webhook
/// receiver mounts one literal route, so the template *is* the path, and the
/// worker derives it from the endpoint's URL rather than asking for it twice.
///
/// ```
/// use sms_webhook::delivery_route;
///
/// assert_eq!(
///     delivery_route("https://api.vaam.store/webhooks/vsms").unwrap(),
///     "/webhooks/vsms"
/// );
/// // A path with a parameter, a query or credentials is not one literal route.
/// assert!(delivery_route("https://example.com/hooks/{tenant}").is_err());
/// assert!(delivery_route("https://example.com/hooks?x=1").is_err());
/// assert!(delivery_route("https://user:pw@example.com/hooks").is_err());
/// assert!(delivery_route("ftp://example.com/hooks").is_err());
/// ```
///
/// # Errors
///
/// [`SealError::Url`] for a URL that is not `http(s)`, carries credentials, a
/// query or a fragment, or has a path with `{`, `}` or `%` in it.
pub fn delivery_route(url: &str) -> Result<String, SealError> {
    let parsed = Url::parse(url).map_err(|_| SealError::Url("not a URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(SealError::Url("the scheme is not http or https"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(SealError::Url("it carries credentials"));
    }
    if parsed.query().is_some() {
        return Err(SealError::Url("it carries a query"));
    }
    if parsed.fragment().is_some() {
        return Err(SealError::Url("it carries a fragment"));
    }
    let path = parsed.path();
    if path.contains(['{', '}', '%']) {
        return Err(SealError::Url("the path is not one literal route"));
    }
    Ok(path.to_owned())
}

/// One delivery, sealed: the request body and every header the seal depends on.
/// Send both as given; add nothing that changes the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedDelivery {
    /// The `COSE_Sign1` message.
    pub body: Vec<u8>,
    /// `Content-Type`, `Accept`, `Cratestack-Contract`, `Cratestack-Payload-Type`
    /// and `Cratestack-Payload-Accept`, as `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
}

/// Seals deliveries under vsms's envelope signing key.
///
/// It is cheap to clone (the envelope inside is shared) and holds no per-request
/// state: every [`Sealer::seal`] draws a fresh nonce and reads the clock, so a
/// retry is a new request, not a resend.
///
/// ```
/// use cratestack::cose::Ed25519Signer;
/// use sms_webhook::Sealer;
///
/// let sealer = Sealer::new(Ed25519Signer::from_seed(&[7; 32])).unwrap();
/// let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// let url = "https://api.vaam.store/webhooks/vsms";
/// let sealed = runtime
///     .block_on(sealer.seal("vaam", url, br#"{"id":"a"}"#))
///     .unwrap();
/// assert!(sealed
///     .headers
///     .iter()
///     .any(|(name, value)| name == "content-type" && value.starts_with("application/cose")));
/// // The signature, the nonce and the clock are in the body, so a second seal differs.
/// let again = runtime
///     .block_on(sealer.seal("vaam", url, br#"{"id":"a"}"#))
///     .unwrap();
/// assert_ne!(sealed.body, again.body);
/// ```
#[derive(Clone)]
pub struct Sealer {
    cose: CoseEnvelope,
}

impl std::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key.
        f.debug_struct("Sealer").finish_non_exhaustive()
    }
}

impl Sealer {
    /// A sealer signing with `signer`, vsms's own envelope signing key.
    ///
    /// # Errors
    ///
    /// [`SealError::Seal`] when the envelope refuses the signer.
    pub fn new(signer: Ed25519Signer) -> Result<Self, SealError> {
        // A delivery opens no answer, so the envelope pins no key to open one
        // with: whatever comes back is not read (`lib.md`, "The answer").
        let cose = CoseEnvelope::client(
            CoseMode::Sign1,
            std::sync::Arc::new(signer),
            std::sync::Arc::new(StaticVerifierResolver::new()),
        )
        .build()
        .map_err(|error| SealError::Seal(error.to_string()))?;
        Self::from_envelope(cose)
    }

    /// A sealer over an envelope the caller built, to pin the clock and the
    /// nonce in a test vector.
    ///
    /// # Errors
    ///
    /// [`SealError::Seal`] for an envelope that is not client-role.
    pub fn from_envelope(cose: CoseEnvelope) -> Result<Self, SealError> {
        if cose.role() != CoseRole::Client {
            return Err(SealError::Seal(
                "a webhook sealer needs a client-role envelope".to_owned(),
            ));
        }
        Ok(Self { cose })
    }

    /// Seals `body` (the envelope JSON) for the endpoint at `url`, addressed to
    /// `audience`.
    ///
    /// The seal binds the audience, `POST`, the route [`delivery_route`] finds in
    /// `url`, [`CONTRACT_SHA`], the payload type and a fresh nonce and
    /// timestamp. It asks to be answered in JSON.
    ///
    /// # Errors
    ///
    /// [`SealError::NoAudience`] for an empty audience, [`SealError::Url`] for a
    /// URL that is not a route, [`SealError::Seal`] when the signer fails.
    pub async fn seal(
        &self,
        audience: &str,
        url: &str,
        body: &[u8],
    ) -> Result<SealedDelivery, SealError> {
        if audience.is_empty() {
            return Err(SealError::NoAudience);
        }
        let route = delivery_route(url)?;
        let envelope = ClientEnvelope::new(self.cose.clone(), audience.to_owned())
            .map_err(|error| SealError::Seal(error.to_string()))?;
        let call = SealCall::new(METHOD, RouteRef::new(&route, &[]), CONTRACT_SHA)
            .payload(body, PAYLOAD_TYPE)
            .accept(PAYLOAD_TYPE);
        let sealed = envelope
            .seal_call(call)
            .await
            .map_err(|error: ClientError| SealError::Seal(error.to_string()))?;
        let headers = sealed
            .headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
            })
            .collect();
        Ok(SealedDelivery {
            body: sealed.body.to_vec(),
            headers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_literal_digest_is_the_hash_of_the_document() {
        assert_eq!(contract_sha(), CONTRACT_SHA);
        let hex = CONTRACT_SHA.iter().fold(String::new(), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        });
        assert_eq!(hex, CONTRACT_SHA_HEX);
    }

    #[test]
    fn the_document_is_valid_json_naming_the_envelope_fields() {
        let doc: serde_json::Value = serde_json::from_str(CONTRACT_DOCUMENT).unwrap();
        let body = &doc["request"]["body"];
        for field in ["id", "eventId", "type", "occurredAt", "data"] {
            assert!(body.get(field).is_some(), "{field}");
        }
        assert_eq!(doc["method"], METHOD);
        assert_eq!(doc["request"]["payload_type"], PAYLOAD_TYPE);
    }

    #[test]
    fn a_route_is_the_path_and_nothing_else() {
        assert_eq!(delivery_route("http://h:8080/a/b").unwrap(), "/a/b");
        assert_eq!(delivery_route("https://h").unwrap(), "/");
        for bad in [
            "https://h/{x}",
            "https://h/a%2Fb",
            "https://h/a?b",
            "https://h/a#b",
            "https://u@h/a",
            "file:///etc",
            "not a url",
        ] {
            assert!(delivery_route(bad).is_err(), "{bad}");
        }
    }
}
