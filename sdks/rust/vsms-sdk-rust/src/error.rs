//! The one error type this SDK returns. Wraps the generated client's own
//! `ClientError` rather than re-inventing a parallel taxonomy, plus the
//! handful of failure modes specific to the auth layer this crate hand-
//! writes (key loading, assertion signing, the token endpoint itself).

use cratestack::client_rust::ClientError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SdkError {
    /// A REST call to vsms failed — transport, codec, or a non-2xx
    /// response. See `cratestack::client_rust::ClientError`'s own variants
    /// for the detail; `Remote { status, .. }` is what callers usually
    /// want to inspect.
    #[error(transparent)]
    Client(#[from] ClientError),

    /// The private key could not be read from disk.
    #[error("reading the private key at {path}: {source}")]
    PrivateKeyRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// The private key was read but is not a PEM `jsonwebtoken` can use to
    /// sign RS256.
    #[error("private key at {path} is not a valid RSA PEM (PKCS#1 or PKCS#8): {source}")]
    PrivateKeyInvalid {
        path: String,
        #[source]
        source: jsonwebtoken::errors::Error,
    },

    /// Signing the RFC 7523 client assertion failed.
    #[error("signing the client assertion: {0}")]
    AssertionSigning(#[source] jsonwebtoken::errors::Error),

    /// The `POST {issuer}/token` exchange itself failed — a transport
    /// error, a non-2xx status, or a response body that didn't parse as
    /// the OAuth2 token response shape. Kept distinct from `Client` above
    /// because `/token` is not a `cratestack`-routed endpoint (it's
    /// `authkestra-op`'s own route) and so never produces a
    /// `cratestack::client_rust::ClientError` in the first place — this
    /// SDK's token store talks to it with a bare `reqwest::Client`.
    #[error("token request to {endpoint} failed: {message}")]
    TokenRequest { endpoint: String, message: String },

    /// A deliberate, small, closed set of configuration mistakes this SDK
    /// can catch itself, rather than let surface as an opaque transport or
    /// parse error three calls later.
    #[error("invalid vsms SDK configuration: {0}")]
    Config(String),
}

impl SdkError {
    /// `true` for a `409 Conflict` — but that status is shared by **two
    /// independent dedupe layers**, not one, and this method can't tell
    /// them apart on its own:
    ///
    /// - a duplicate `SendMessageInput::clientRef` (`messages_app_idem_key`'s
    ///   database-level unique index, scoped by `App`) — the caller *chose*
    ///   to send the same logical message twice;
    /// - an `Idempotency-Key` still `IdempotencyLayer` is holding a
    ///   reservation for (`"another request with this Idempotency-Key is
    ///   still in flight"` — a genuinely concurrent duplicate request under
    ///   the same key, not a replay, since a replay returns the original
    ///   status instead of `409`).
    ///
    /// If a call passed an `idempotency_key`, check
    /// [`SdkError::is_idempotency_in_flight`] first — its `Display` text
    /// (the raw response body when the body isn't JSON, per
    /// `decode_send_message_response`'s own doc) is what actually
    /// distinguishes the two, and conflating them in a message an
    /// integrator reads would be worse than saying nothing. See
    /// `examples/rust/sms-send`'s README for the same distinction, spelled
    /// out for a human reading the example rather than matching on this
    /// method.
    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            SdkError::Client(ClientError::Remote { status, .. })
                if status.as_u16() == 409
        )
    }

    /// `true` for the specific `409` `IdempotencyLayer` returns while
    /// another request under the *same* `Idempotency-Key` is still being
    /// processed (not yet reserved-and-replayable). Distinguished from
    /// `clientRef`'s `409` (see [`SdkError::is_conflict`]) by message
    /// content — that response body is plain text, not a `CratestackErrorResponse`,
    /// so `decode_send_message_response` falls back to using it verbatim as
    /// this error's message, and it always contains "Idempotency-Key".
    pub fn is_idempotency_in_flight(&self) -> bool {
        matches!(
            self,
            SdkError::Client(ClientError::Remote { status, message, .. })
                if status.as_u16() == 409 && message.contains("Idempotency-Key")
        )
    }

    /// `true` for the `422` `IdempotencyLayer` returns when an
    /// `Idempotency-Key` is reused with a request that doesn't match the
    /// first one under that key byte-for-byte (a different body, in
    /// practice — see `cratestack_axum::idempotency`'s own doc). This is a
    /// caller bug, not a race: fix the key or the request, not a retry.
    pub fn is_idempotency_key_conflict(&self) -> bool {
        matches!(
            self,
            SdkError::Client(ClientError::Remote { status, message, .. })
                if status.as_u16() == 422 && message.contains("idempotency_key_conflict")
        )
    }

    /// `true` if the final error surfaced from a call was a `401
    /// Unauthorized` — i.e. `VsmsClient`'s bounded refresh-on-401 tried
    /// once, invalidated the cached token, retried, and was rejected
    /// again. Mostly useful for tests asserting that bound actually holds.
    pub fn is_unauthorized(&self) -> bool {
        matches!(
            self,
            SdkError::Client(ClientError::Remote { status, .. })
                if status.as_u16() == 401
        )
    }
}
