//! The one error type this SDK returns. Wraps the generated client's own
//! `ClientError` rather than re-inventing a parallel taxonomy, plus the
//! handful of failure modes specific to the signing key and the pinned server
//! keys this crate hand-writes.

use cratestack::client_rust::{ClientError, EnvelopeError};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SdkError {
    /// A call to vsms failed - transport, the signed transport, codec, or a
    /// non-2xx response. See `cratestack::client_rust::ClientError`'s own
    /// variants for the detail; `Remote { status, .. }` is what callers
    /// usually want to inspect, and `Envelope(Unsigned { status: 401 })` is how
    /// a refused signature looks (see [`SdkError::is_unauthorized`]).
    #[error(transparent)]
    Client(#[from] ClientError),

    /// The signing key could not be read or is not a PKCS#8 Ed25519 key. The
    /// text never quotes the key.
    #[error("signing key: {0}")]
    Key(String),

    /// A deliberate, small, closed set of configuration mistakes this SDK
    /// can catch itself, rather than let surface as an opaque transport or
    /// parse error three calls later.
    #[error("invalid vsms SDK configuration: {0}")]
    Config(String),
}

impl SdkError {
    /// `true` for a `409 Conflict` - but that status is shared by **two
    /// independent dedupe layers**, not one, and this method can't tell
    /// them apart on its own:
    ///
    /// - a duplicate `SendMessageInput::clientRef` (`messages_app_idem_key`'s
    ///   database-level unique index, scoped by `App`) - the caller *chose*
    ///   to send the same logical message twice;
    /// - an `Idempotency-Key` the idempotency layer is still holding a
    ///   reservation for (a genuinely concurrent duplicate request under the
    ///   same key, not a replay, since a replay returns the original status
    ///   instead of `409`).
    ///
    /// If a call passed an `idempotency_key`, check
    /// [`SdkError::is_idempotency_in_flight`] first.
    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            SdkError::Client(ClientError::Remote { status, .. })
                if status.as_u16() == 409
        )
    }

    /// `true` for the specific `409` the idempotency layer returns while
    /// another request under the *same* `Idempotency-Key` is still being
    /// processed. Distinguished from `clientRef`'s `409` (see
    /// [`SdkError::is_conflict`]) by message content: it always names the
    /// `Idempotency-Key`.
    pub fn is_idempotency_in_flight(&self) -> bool {
        matches!(
            self,
            SdkError::Client(ClientError::Remote { status, message, .. })
                if status.as_u16() == 409 && message.contains("Idempotency-Key")
        )
    }

    /// `true` for the `422` the idempotency layer returns when an
    /// `Idempotency-Key` is reused with a request that doesn't match the
    /// first one under that key byte-for-byte (a different body, in
    /// practice). This is a caller bug, not a race: fix the key or the
    /// request, not a retry.
    pub fn is_idempotency_key_conflict(&self) -> bool {
        matches!(
            self,
            SdkError::Client(ClientError::Remote { status, message, .. })
                if status.as_u16() == 422 && message.contains("idempotency_key_conflict")
        )
    }

    /// `true` when the gateway did not accept the signature: the unsigned
    /// `401` it answers to an unknown or revoked key, an inactive client, a
    /// stale `iat` (check the clock), a replayed request or a bad signature.
    /// It is deliberately one answer for all of them, so the SDK can say no
    /// more; the first things to check are that the key's public half is
    /// registered and not revoked, and that this machine's clock is within
    /// five minutes of the server's. The SDK does **not** retry it: a retry
    /// is a new request with a new nonce, the caller's decision.
    pub fn is_unauthorized(&self) -> bool {
        self.status() == Some(401)
    }

    /// The HTTP status behind this error, when there is one: a sealed remote
    /// error's, or the status of an unsigned refusal.
    pub fn status(&self) -> Option<u16> {
        match self {
            SdkError::Client(ClientError::Remote { status, .. }) => Some(status.as_u16()),
            SdkError::Client(ClientError::Envelope(EnvelopeError::Unsigned { status })) => {
                Some(*status)
            }
            _ => None,
        }
    }
}
