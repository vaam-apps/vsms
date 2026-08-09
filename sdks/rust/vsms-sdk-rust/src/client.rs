//! [`VsmsClient`] — a thin wrapper around the generated
//! `schema::client::Client` that wires in the auth layer
//! ([`token`](crate::token), [`authorizer`](crate::authorizer)) and adds
//! the one behaviour issue #171 requires that neither the generated client
//! nor `cratestack::client_rust::RequestAuthorizer` provide on their own:
//! a bounded refresh-on-401. `RequestAuthorizer::authorize` runs *before*
//! a request, with no visibility into its response, so "the token I
//! attached was rejected, mint one refresh and try again" has to live
//! here, one layer up.
//!
//! Everything else this struct exposes (`send_message`, `get_message`)
//! is a direct, undecorated call into generated code — there is no
//! hand-duplicated request/response type anywhere in this file.

use std::future::Future;
use std::sync::Arc;

use cratestack::client_rust::{
    ClientConfig, ClientError, CratestackClient, JsonCodec, RuntimeHeader, RuntimeRequestWire,
    RuntimeResponseWire,
};
use cratestack::{CoolError, CoolErrorResponse};
use reqwest::StatusCode;

use crate::authorizer::GatewayAuthorizer;
use crate::error::SdkError;
use crate::schema;
use crate::token::{PrivateKeyJwtConfig, PrivateKeyJwtTokenStore, TokenStore};

/// Response header name `IdempotencyLayer` (`crates/sms-api/src/router.rs`,
/// `cratestack_axum::idempotency`) appends to a replayed response.
/// `HeaderMap`/`RuntimeHeader` comparisons are case-insensitive per HTTP,
/// so the exact case here doesn't have to match the wire byte-for-byte.
const IDEMPOTENCY_REPLAYED_HEADER: &str = "idempotency-replayed";
const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
const SEND_MESSAGE_PATH: &str = "/$procs/sendMessage";

/// Installs a `ring`-backed `rustls::crypto::CryptoProvider` if the
/// process doesn't already have one, mirroring
/// `cratestack-client-rust`'s own `client/core.rs::ensure_crypto_provider`
/// (that one only covers the `reqwest::Client` `CratestackClient` builds
/// internally; this crate also builds its own, separate `reqwest::Client`
/// for the `/token` exchange in `token.rs`, which needs the same
/// courtesy-install). Idempotent and safe to call from both places —
/// whichever runs first wins, and a consumer that installed a different
/// backend (including `aws-lc-rs`) before touching this crate keeps that
/// choice, matching `cratestack-client-rust`'s own documented contract.
pub(crate) fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// A vsms client: token acquisition, attachment, and bounded refresh are
/// all handled internally. Build one with [`VsmsClient::private_key_jwt`]
/// for the common case, or [`VsmsClient::builder`] for anything else (a
/// pre-built [`TokenStore`], a custom `reqwest::Client`, ...).
pub struct VsmsClient {
    inner: schema::client::Client<JsonCodec>,
    token_store: Arc<dyn TokenStore>,
}

impl VsmsClient {
    pub fn builder() -> VsmsClientBuilder {
        VsmsClientBuilder::default()
    }

    /// The common case in one call: `private_key_jwt` against a key on
    /// disk (or supplied in memory — see [`PrivateKeyJwtConfig::new`]),
    /// talking to `base_url` for both the OIDC `/token` endpoint and the
    /// REST API (the same origin in every vsms deployment today — see
    /// `examples/rust/sms-send`'s own `--issuer`).
    pub fn private_key_jwt(
        base_url: impl Into<String>,
        config: PrivateKeyJwtConfig,
    ) -> Result<Self, SdkError> {
        Self::builder()
            .base_url(base_url)
            .private_key_jwt(config)?
            .build()
    }

    /// The token store backing this client — mostly useful for tests that
    /// want to assert on cache behaviour (e.g. counting `/token` requests
    /// against a real gateway) without threading a second handle through
    /// the caller's own code.
    pub fn token_store(&self) -> &Arc<dyn TokenStore> {
        &self.token_store
    }

    /// Escape hatch to the generated client for anything this wrapper
    /// doesn't (yet) expose a convenience method for — `list_messages_page`,
    /// `cancel_message`, `preview_message`, model CRUD, ... The auth layer
    /// (and the bounded-refresh-on-401 behaviour below) applies here too,
    /// since it's the same `CratestackClient` underneath; callers only
    /// lose the automatic retry, not the authorization header.
    pub fn generated(&self) -> &schema::client::Client<JsonCodec> {
        &self.inner
    }

    /// `POST /$procs/sendMessage`. The three-step HTTP dance
    /// (`examples/rust/sms-send`'s previous, hand-rolled version: sign an
    /// assertion, exchange it, attach the Bearer token) collapses to this
    /// one call — the token lifecycle is handled internally, including a
    /// bounded refresh if the attached token is unexpectedly rejected.
    ///
    /// `idempotency_key`, when `Some`, is sent as the `Idempotency-Key`
    /// request header — `IdempotencyLayer` (#153,
    /// `crates/sms-api/src/router.rs`) then guarantees a repeated call
    /// under the same key within its TTL (24h by default) never
    /// re-executes `sendMessage`: it replays the first response verbatim,
    /// no second SMS, no second `Message` row. That is a *different*
    /// safety net from `SendMessageInput::clientRef` (a database-level
    /// unique index scoped by `App`, checked in this same crate's
    /// `SdkError::is_conflict`): `clientRef` protects against the caller
    /// choosing to send the same logical message twice on purpose;
    /// `Idempotency-Key` protects against the caller not knowing whether
    /// their *previous HTTP request* actually landed — the exact
    /// asymmetry `ProviderError::Indeterminate` (#119) already documents
    /// one hop down (a timed-out submit must never be blindly retried,
    /// because the request may have already been accepted). This SDK's
    /// own `refresh_once_on_401` retry is safe without an idempotency key
    /// — a `401` means the request was rejected before `sendMessage` ever
    /// ran — but a *caller's* retry after a timeout or a dropped
    /// connection is exactly the case `Idempotency-Key` exists for, and
    /// an SDK is the layer that should make passing one the easy thing to
    /// do, not an afterthought bolted onto `reqwest` by hand.
    ///
    /// Returns [`SendMessageOutcome`], not a bare `SendMessageResult` —
    /// `idempotency_replayed` tells the caller whether this response was
    /// actually executed or replayed from a prior call under the same
    /// key, information the generated client's typed `procedures().
    /// send_message()` method has no way to surface at all (`crud.rs`'s
    /// `decode_typed_response` discards response headers once it decodes
    /// the body). That's why this method goes around the generated
    /// procedure call and uses `CratestackClient::execute_raw_transport`
    /// directly — still through the same authenticated `CratestackClient`
    /// (the request authorizer still runs), just not through the
    /// header-discarding typed wrapper.
    pub async fn send_message(
        &self,
        args: schema::SendMessageInput,
        idempotency_key: Option<&str>,
    ) -> Result<SendMessageOutcome, SdkError> {
        let wire_args = schema::procedures::send_message::Args { args };
        let body = serde_json::to_vec(&wire_args).map_err(|error| {
            SdkError::Client(ClientError::Codec(CoolError::Codec(format!(
                "failed to encode sendMessage args as JSON: {error}"
            ))))
        })?;

        // No explicit `Accept` override needed as of cratestack 0.7.10:
        // cratestack/cratestack#489 made server-side response negotiation
        // codec-aware (`select_transport_response_content_type` now filters
        // candidates through what the router's actual `HttpTransport` can
        // encode), so a `JsonCodec`-only router like `sms-gateway`'s no
        // longer 406s on the generated client's default `Accept:
        // application/json, application/cbor` — it correctly falls back to
        // JSON, the one codec it can actually produce. See `get_message`'s
        // call below, which drops the equivalent per-call override too.
        let mut headers = Vec::new();
        if let Some(key) = idempotency_key {
            headers.push(RuntimeHeader {
                name: IDEMPOTENCY_KEY_HEADER.to_owned(),
                value: key.to_owned(),
            });
        }

        let build_request = || RuntimeRequestWire {
            method: "POST".to_owned(),
            path: SEND_MESSAGE_PATH.to_owned(),
            canonical_query: None,
            headers: headers.clone(),
            body: body.clone(),
        };

        let runtime = self.inner.runtime();
        let response = match runtime.execute_raw_transport(build_request()).await {
            Ok(response) if response.status_code == StatusCode::UNAUTHORIZED.as_u16() => {
                self.token_store.invalidate().await;
                runtime.execute_raw_transport(build_request()).await?
            }
            Ok(response) => response,
            Err(error) => return Err(SdkError::from(error)),
        };

        decode_send_message_response(response)
    }

    /// `GET /messages/{id}` — reads a message back, e.g. to confirm a
    /// send actually landed rather than trusting the mutation's own
    /// echoed response (the same proof `examples/rust/sms-send` has
    /// always done).
    pub async fn get_message(&self, id: &str) -> Result<schema::Message, SdkError> {
        let messages = self.inner.messages();
        let id = id.to_owned();
        self.refresh_once_on_401(|| messages.get(&id, &[])).await
    }

    /// Runs `make_call`; if it fails with `401 Unauthorized`, invalidates
    /// the cached token and runs it exactly once more before surfacing
    /// whatever the retry produces. Any other error (including a *second*
    /// 401) is returned as-is — this is deliberately bounded to one
    /// refresh, never a loop, because `/token` has no rate limiting today
    /// (#156) and an unbounded refresh loop against it would be a self-
    /// inflicted denial of service.
    async fn refresh_once_on_401<T, Fut>(
        &self,
        mut make_call: impl FnMut() -> Fut,
    ) -> Result<T, SdkError>
    where
        Fut: Future<Output = Result<T, ClientError>>,
    {
        match make_call().await {
            Ok(value) => Ok(value),
            Err(ClientError::Remote { status, .. }) if status == StatusCode::UNAUTHORIZED => {
                self.token_store.invalidate().await;
                make_call().await.map_err(SdkError::from)
            }
            Err(other) => Err(SdkError::from(other)),
        }
    }
}

/// [`VsmsClient::send_message`]'s return type: the decoded
/// `SendMessageResult` plus whether this response was replayed from a
/// prior call under the same `Idempotency-Key` rather than freshly
/// executed. `idempotency_replayed` is always `false` when no
/// `idempotency_key` was passed — nothing to replay against.
#[derive(Debug, Clone, PartialEq)]
pub struct SendMessageOutcome {
    pub result: schema::SendMessageResult,
    pub idempotency_replayed: bool,
}

/// Decodes a raw `sendMessage` response, mirroring
/// `cratestack_client_rust::client::decode::decode_typed_response`'s own
/// status-based branch (that function is `pub(crate)` there, unreachable
/// from this crate, hence the small duplication here) with one real
/// improvement: on a non-2xx response whose body isn't JSON — the
/// idempotency in-flight response (`409`, `text/plain`, "another request
/// with this Idempotency-Key is still in flight") is exactly this shape —
/// the raw body text becomes the error message instead of a generic
/// "unexpected error body" placeholder, so that message reaches the
/// caller rather than being silently dropped.
fn decode_send_message_response(
    response: RuntimeResponseWire,
) -> Result<SendMessageOutcome, SdkError> {
    let idempotency_replayed = response.headers.iter().any(|header| {
        header
            .name
            .eq_ignore_ascii_case(IDEMPOTENCY_REPLAYED_HEADER)
            && header.value.eq_ignore_ascii_case("true")
    });

    if (200..=299).contains(&response.status_code) {
        let result: schema::SendMessageResult =
            serde_json::from_slice(&response.body).map_err(|error| {
                SdkError::Client(ClientError::Codec(CoolError::Codec(format!(
                    "failed to decode sendMessage response as JSON: {error}"
                ))))
            })?;
        return Ok(SendMessageOutcome {
            result,
            idempotency_replayed,
        });
    }

    let parsed: Option<CoolErrorResponse> = serde_json::from_slice(&response.body).ok();
    let message = match &parsed {
        Some(error) => error.message.clone(),
        None if !response.body.is_empty() => {
            String::from_utf8_lossy(&response.body).trim().to_owned()
        }
        None => format!("unexpected error body for status {}", response.status_code),
    };
    let status =
        StatusCode::from_u16(response.status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    Err(SdkError::Client(ClientError::Remote {
        status,
        error: parsed,
        message,
    }))
}

/// Builds a [`VsmsClient`]. `base_url` and a token store (most commonly
/// via [`VsmsClientBuilder::private_key_jwt`]) are the only required
/// pieces.
#[derive(Default)]
pub struct VsmsClientBuilder {
    base_url: Option<String>,
    token_store: Option<Arc<dyn TokenStore>>,
    http: Option<reqwest::Client>,
}

impl VsmsClientBuilder {
    /// The gateway's externally reachable origin.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Use a caller-supplied [`TokenStore`] — a hand-rolled test double,
    /// or a `private_key_jwt` store already built with a `reqwest::Client`
    /// this builder doesn't know about.
    pub fn token_store(mut self, token_store: Arc<dyn TokenStore>) -> Self {
        self.token_store = Some(token_store);
        self
    }

    /// Builds a [`PrivateKeyJwtTokenStore`] from `config` and uses it as
    /// this client's token store — the common path.
    pub fn private_key_jwt(mut self, config: PrivateKeyJwtConfig) -> Result<Self, SdkError> {
        self.token_store = Some(Arc::new(PrivateKeyJwtTokenStore::new(config)?));
        Ok(self)
    }

    /// Reuse an existing `reqwest::Client` for the REST/procedure calls
    /// (not the `/token` exchange, which `PrivateKeyJwtConfig::http`
    /// covers separately — the two are independent HTTP clients, matching
    /// how `token.ts` and `examples/rust/sms-send` have always kept them).
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    pub fn build(self) -> Result<VsmsClient, SdkError> {
        let base_url = self
            .base_url
            .ok_or_else(|| SdkError::Config("base_url is required".to_owned()))?;
        let token_store = self.token_store.ok_or_else(|| {
            SdkError::Config(
                "a token store is required — call .private_key_jwt(...) or .token_store(...)"
                    .to_owned(),
            )
        })?;
        let url = reqwest::Url::parse(&base_url)
            .map_err(|error| SdkError::Config(format!("invalid base_url '{base_url}': {error}")))?;

        ensure_crypto_provider();
        let runtime = match self.http {
            Some(http) => {
                CratestackClient::with_http_client(ClientConfig::new(url), JsonCodec, http)
            }
            None => CratestackClient::new(ClientConfig::new(url), JsonCodec),
        }
        .with_request_authorizer(Arc::new(GatewayAuthorizer::new(token_store.clone())));

        Ok(VsmsClient {
            inner: schema::client::Client::new(runtime),
            token_store,
        })
    }
}
