//! [`VsmsClient`] - a thin wrapper around the generated
//! `schema::client::Client` that seals every call.
//!
//! Every request is a COSE_Sign1 message signed by the caller's
//! [`SigningKey`](crate::SigningKey), and every answer is opened against the
//! gateway's pinned [`ServerKeys`](crate::ServerKeys): the same envelope on the
//! generated client and on `send_message`, built once. There is no token, no
//! refresh and no retry: a request carries a fresh nonce the gateway records,
//! so replaying the bytes is refused, and re-sending is the caller's decision.
//!
//! Everything the generated client does it still does (`generated()`); the one
//! call that goes around it is `send_message`, which needs the response
//! headers the typed wrapper discards.

use cratestack::client_rust::{
    ClientConfig, ClientEnvelope, ClientError, CratestackClient, JsonCodec, RouteRef, SealCall,
};
use cratestack::cose::{CoseEnvelope, CoseMode, CoseVerifyKey, StaticVerifierResolver};
use cratestack::{CratestackError, CratestackErrorResponse};
use reqwest::StatusCode;
use std::sync::Arc;

use crate::error::SdkError;
use crate::schema;
use crate::server_keys::ServerKeys;
use crate::signer::SigningKey;

/// The audience every request binds: the gateway's configured name, never its
/// host (cratestack ADR 0006 section 4). It is a constant of the protocol, not
/// of the deployment, so a request sealed for another service fails there.
pub const AUDIENCE: &str = "vsms";

/// The only payload type the gateway seals, in both directions.
const PAYLOAD_TYPE: &str = "application/json";

/// Response header the idempotency layer appends to a replayed response.
/// Headers are not part of the seal, so this one is a hint: see
/// [`SendMessageOutcome::idempotency_replayed`].
const IDEMPOTENCY_REPLAYED_HEADER: &str = "idempotency-replayed";
const SEND_MESSAGE_ROUTE: &str = "/$procs/sendMessage";

/// Installs a `ring`-backed `rustls::crypto::CryptoProvider` if the process
/// doesn't already have one, mirroring `cratestack-client-rust`'s own
/// `client/core.rs::ensure_crypto_provider` (that one only covers the
/// `reqwest::Client` `CratestackClient` builds internally; this crate also
/// builds its own for `send_message`). Idempotent and safe to call from both
/// places - whichever runs first wins, and a consumer that installed a
/// different backend before touching this crate keeps that choice.
pub(crate) fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// A vsms client. Build one with [`VsmsClient::signed`] for the common case,
/// or [`VsmsClient::builder`] for a custom `reqwest::Client`.
pub struct VsmsClient {
    inner: schema::client::Client<JsonCodec>,
    envelope: ClientEnvelope,
    http: reqwest::Client,
    base_url: String,
}

impl VsmsClient {
    pub fn builder() -> VsmsClientBuilder {
        VsmsClientBuilder::default()
    }

    /// The common case in one call: a gateway origin, the caller's signing
    /// key and the gateway's pinned keys.
    ///
    /// ```no_run
    /// # fn demo() -> Result<(), vsms_sdk::SdkError> {
    /// use vsms_sdk::{ServerKeys, SigningKey, VsmsClient};
    ///
    /// let client = VsmsClient::signed(
    ///     "https://sms.example.com",
    ///     SigningKey::from_pem_file("/run/secrets/vsms-signing-key.pem")?,
    ///     // From `sms-gateway server-keys`; public, so plain configuration.
    ///     ServerKeys::parse_report(&std::fs::read_to_string("/etc/vsms/server-keys.txt").unwrap())?,
    /// )?;
    /// # let _ = client;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`SdkError::Config`] for an unusable `base_url`.
    pub fn signed(
        base_url: impl Into<String>,
        signing_key: SigningKey,
        server_keys: ServerKeys,
    ) -> Result<Self, SdkError> {
        Self::builder()
            .base_url(base_url)
            .signing_key(signing_key)
            .server_keys(server_keys)
            .build()
    }

    /// Escape hatch to the generated client for anything this wrapper
    /// doesn't (yet) expose a convenience method for - `list_messages_page`,
    /// `cancel_message`, `preview_message`, model CRUD, ... It is sealed by
    /// the same envelope, so every call is signed and every answer verified.
    pub fn generated(&self) -> &schema::client::Client<JsonCodec> {
        &self.inner
    }

    /// `POST /$procs/sendMessage`, sealed.
    ///
    /// `idempotency_key`, when `Some`, is sent as the `Idempotency-Key`
    /// request header, which the seal binds - the idempotency layer then
    /// guarantees a repeated call under the same key within its TTL (24h by
    /// default) never re-executes `sendMessage`: it replays the first response,
    /// no second SMS, no second `Message` row. Each attempt is a freshly
    /// sealed request (a new nonce), which the layer inside the gateway's
    /// envelope accepts under the same key. That is a *different* safety net
    /// from `SendMessageInput::clientRef` (a database-level unique index
    /// scoped by `App`): `clientRef` protects against the caller choosing to
    /// send the same logical message twice on purpose; `Idempotency-Key`
    /// protects against the caller not knowing whether their *previous HTTP
    /// request* actually landed - the exact asymmetry
    /// `ProviderError::Indeterminate` documents one hop down.
    ///
    /// Returns [`SendMessageOutcome`], not a bare `SendMessageResult`, because
    /// `idempotency_replayed` is read from a response header, which the
    /// generated client's typed `procedures().send_message()` discards. That
    /// is why this method seals the call itself (`ClientEnvelope::seal_call`)
    /// and sends it with its own `reqwest` call, then opens the answer against
    /// the pinned server keys.
    pub async fn send_message(
        &self,
        args: schema::SendMessageInput,
        idempotency_key: Option<&str>,
    ) -> Result<SendMessageOutcome, SdkError> {
        let wire_args = schema::procedures::send_message::Args { args };
        let body = serde_json::to_vec(&wire_args).map_err(|error| {
            SdkError::Client(ClientError::Codec(CratestackError::Codec(format!(
                "failed to encode sendMessage args as JSON: {error}"
            ))))
        })?;

        let contract = *cratestack::find_contract(schema::OP_CONTRACTS, "POST", SEND_MESSAGE_ROUTE)
            .ok_or_else(|| {
                SdkError::Config("the vendored schema has no sendMessage op".to_owned())
            })?;
        let mut call = SealCall::new("POST", RouteRef::new(SEND_MESSAGE_ROUTE, &[]), contract)
            .payload(&body, PAYLOAD_TYPE)
            .accept(PAYLOAD_TYPE);
        if let Some(key) = idempotency_key {
            call = call.idempotency_key(key);
        }
        let sealed = self.envelope.seal_call(call).await?;

        let mut request = self
            .http
            .post(format!("{}{SEND_MESSAGE_ROUTE}", self.base_url))
            .body(sealed.body);
        for (name, value) in &sealed.headers {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(ClientError::from)?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await.map_err(ClientError::from)?;

        // An unsigned answer, or one that fails verification, is an error
        // whatever its status: nothing in its body is read.
        let opened = sealed
            .pending
            .open(status.as_u16(), &headers, bytes)
            .await?;
        let replayed = headers
            .get(IDEMPOTENCY_REPLAYED_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("true"));
        decode_send_message_response(status, &opened.body, replayed)
    }

    /// `GET /messages/{id}` - reads a message back, e.g. to confirm a send
    /// actually landed rather than trusting the mutation's own echoed
    /// response.
    pub async fn get_message(&self, id: &str) -> Result<schema::Message, SdkError> {
        let id = id.to_owned();
        self.inner
            .messages()
            .get(&id, &[])
            .await
            .map_err(SdkError::from)
    }
}

/// [`VsmsClient::send_message`]'s return type: the decoded
/// `SendMessageResult` plus whether this response was replayed from a prior
/// call under the same `Idempotency-Key` rather than freshly executed.
/// `idempotency_replayed` is always `false` when no `idempotency_key` was
/// passed - nothing to replay against.
///
/// It is read from the `Idempotency-Replayed` response header, and **response
/// headers are not authenticated by the seal** (only the payload, the status
/// and the request they answer are). Treat it as informational: the
/// authoritative statement that a retry did not duplicate the message is that
/// both answers carry the same `messageId`.
#[derive(Debug, Clone, PartialEq)]
pub struct SendMessageOutcome {
    pub result: schema::SendMessageResult,
    pub idempotency_replayed: bool,
}

/// Decodes an opened `sendMessage` answer. A non-2xx answer whose body isn't
/// the standard error JSON becomes an error whose message is the raw text, so
/// what the gateway said reaches the caller instead of a placeholder.
fn decode_send_message_response(
    status: StatusCode,
    body: &[u8],
    idempotency_replayed: bool,
) -> Result<SendMessageOutcome, SdkError> {
    if status.is_success() {
        let result: schema::SendMessageResult = serde_json::from_slice(body).map_err(|error| {
            SdkError::Client(ClientError::Codec(CratestackError::Codec(format!(
                "failed to decode sendMessage response as JSON: {error}"
            ))))
        })?;
        return Ok(SendMessageOutcome {
            result,
            idempotency_replayed,
        });
    }

    let parsed: Option<CratestackErrorResponse> = serde_json::from_slice(body).ok();
    let message = match &parsed {
        Some(error) => error.message.clone(),
        None if !body.is_empty() => String::from_utf8_lossy(body).trim().to_owned(),
        None => format!("unexpected error body for status {status}"),
    };
    Err(SdkError::Client(ClientError::Remote {
        status,
        error: parsed,
        message,
    }))
}

/// Builds a [`VsmsClient`]. `base_url`, a signing key and the pinned server
/// keys are required.
#[derive(Default)]
pub struct VsmsClientBuilder {
    base_url: Option<String>,
    signing_key: Option<SigningKey>,
    server_keys: Option<ServerKeys>,
    http: Option<reqwest::Client>,
}

impl VsmsClientBuilder {
    /// The gateway's externally reachable origin (`https://sms.example.com`).
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// The key every request is signed with.
    pub fn signing_key(mut self, signing_key: SigningKey) -> Self {
        self.signing_key = Some(signing_key);
        self
    }

    /// The gateway's pinned public keys.
    pub fn server_keys(mut self, server_keys: ServerKeys) -> Self {
        self.server_keys = Some(server_keys);
        self
    }

    /// Reuse an existing `reqwest::Client` (a proxy, a timeout, a pool). **It
    /// must not follow redirects**: a sealed request is bound to one route and
    /// one audience, and a hop that already received the sealed bytes cannot
    /// be undone by the check after the fact.
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    pub fn build(self) -> Result<VsmsClient, SdkError> {
        let base_url = self
            .base_url
            .ok_or_else(|| SdkError::Config("base_url is required".to_owned()))?;
        let signing_key = self.signing_key.ok_or_else(|| {
            SdkError::Config("a signing key is required - call .signing_key(...)".to_owned())
        })?;
        let server_keys = self.server_keys.ok_or_else(|| {
            SdkError::Config(
                "the gateway's pinned keys are required - call .server_keys(...) with the \
                 output of `sms-gateway server-keys`"
                    .to_owned(),
            )
        })?;
        let url = reqwest::Url::parse(&base_url)
            .map_err(|error| SdkError::Config(format!("invalid base_url '{base_url}': {error}")))?;

        ensure_crypto_provider();
        let mut resolver = StaticVerifierResolver::new();
        for public in [server_keys.current(), server_keys.next()] {
            resolver = resolver.with_key(
                CoseVerifyKey::ed25519(public)
                    .map_err(|_| SdkError::Config("a pinned server key is invalid".to_owned()))?,
            );
        }
        let cose = CoseEnvelope::client(
            CoseMode::Sign1,
            Arc::new(signing_key.signer()),
            Arc::new(resolver),
        )
        .build()
        .map_err(|error| SdkError::Config(format!("the signed transport: {error}")))?;
        let envelope = ClientEnvelope::new(cose, AUDIENCE)?;

        let http = match self.http {
            Some(http) => http,
            None => reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(ClientError::from)?,
        };
        let runtime =
            CratestackClient::with_http_client(ClientConfig::new(url), JsonCodec, http.clone())
                .with_envelope(envelope.clone())?;

        Ok(VsmsClient {
            inner: schema::client::Client::new(runtime),
            envelope,
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_2xx_answer_decodes_and_carries_the_replay_hint() {
        let body = serde_json::json!({
            "messageId": "m1",
            "state": "queued",
            "encoding": "gsm7",
            "segments": 1,
            "operator": "orange",
            "estimatedCostXaf": "15",
        });
        for replayed in [false, true] {
            let outcome =
                decode_send_message_response(StatusCode::OK, body.to_string().as_bytes(), replayed)
                    .expect("a 2xx answer with a well-formed body is a success");
            assert_eq!(outcome.result.messageId, "m1");
            assert_eq!(outcome.idempotency_replayed, replayed);
        }
    }

    #[test]
    fn a_non_json_error_body_becomes_the_error_message() {
        let error = decode_send_message_response(
            StatusCode::CONFLICT,
            b"another request with this Idempotency-Key is still in flight",
            false,
        )
        .unwrap_err();
        assert!(error.is_conflict());
        assert!(error.is_idempotency_in_flight(), "{error}");
    }

    #[test]
    fn a_json_error_body_keeps_its_message_and_status() {
        let error = decode_send_message_response(
            StatusCode::FORBIDDEN,
            br#"{"code":"FORBIDDEN","message":"missing required permission \"sms:send\""}"#,
            false,
        )
        .unwrap_err();
        assert_eq!(error.status(), Some(403));
        assert!(error.to_string().contains("sms:send"), "{error}");
    }
}
