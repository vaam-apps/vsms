//! The two ways a federated sign-in can stop, and why they are kept apart.

use thiserror::Error;

/// The precise reason a sign-in was **refused**. This is for logs and for
/// tests, never for the browser: the callback answers every refusal with the
/// same generic `access_denied`, so nothing here is an enumeration oracle
/// ("that account exists but is inactive", "you are not in the group").
///
/// Nothing in a variant ever carries a token, a code, a secret, or a claim
/// value - a variant is a fixed label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// The `id_token` is not a three-part JWS this broker can read.
    IdTokenMalformed,
    /// The header `alg` is not an allowed asymmetric algorithm (`none` and
    /// the `HS*` family land here, so does `EdDSA`).
    AlgorithmNotAllowed,
    /// The header carries no `kid`.
    MissingKeyId,
    /// No key in the issuer's JWKS has the header's `kid`, even after the
    /// one permitted refetch.
    UnknownKeyId,
    /// The key found cannot have signed a token with this `alg` (family,
    /// `use`, or `alg` mismatch).
    KeyUnsuitable,
    /// The signature does not verify.
    SignatureInvalid,
    /// `iss` is not the configured issuer, byte for byte.
    IssuerMismatch,
    /// `aud` does not contain this gateway's client id.
    AudienceMismatch,
    /// `azp` is present and not this client, or `aud` has several values and
    /// `azp` is absent.
    AuthorizedPartyMismatch,
    /// `exp` is in the past (beyond the leeway).
    Expired,
    /// `nbf` is in the future (beyond the leeway).
    NotYetValid,
    /// `iat` is in the future (beyond the leeway).
    IssuedInFuture,
    /// A required claim (`exp`, `iss`, `aud`, `sub`, `iat`) is missing or
    /// malformed.
    MissingClaim,
    /// `nonce` is absent or is not the one this transaction sent.
    NonceMismatch,
    /// Neither the `acr` rule nor the `amr` rule is satisfied.
    MfaNotSatisfied,
    /// The user is not in the required group.
    NotInRequiredGroup,
    /// A linked account is deactivated, soft-deleted, or gone.
    UserInactive,
    /// First sign-in, but the token carries no usable email to create the
    /// `User` with.
    EmailMissing,
    /// First sign-in, but a local account already has this email. Linking is
    /// by `(iss, sub)` only - never by email - so this is refused rather
    /// than merged.
    EmailInUse,
    /// The upstream `IdP` itself answered with an error (the user pressed
    /// "cancel", or the realm denied the request).
    UpstreamDenied,
}

impl Refusal {
    /// A stable `snake_case` label for logs and metrics.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::IdTokenMalformed => "id_token_malformed",
            Self::AlgorithmNotAllowed => "algorithm_not_allowed",
            Self::MissingKeyId => "missing_key_id",
            Self::UnknownKeyId => "unknown_key_id",
            Self::KeyUnsuitable => "key_unsuitable",
            Self::SignatureInvalid => "signature_invalid",
            Self::IssuerMismatch => "issuer_mismatch",
            Self::AudienceMismatch => "audience_mismatch",
            Self::AuthorizedPartyMismatch => "authorized_party_mismatch",
            Self::Expired => "expired",
            Self::NotYetValid => "not_yet_valid",
            Self::IssuedInFuture => "issued_in_future",
            Self::MissingClaim => "missing_claim",
            Self::NonceMismatch => "nonce_mismatch",
            Self::MfaNotSatisfied => "mfa_not_satisfied",
            Self::NotInRequiredGroup => "not_in_required_group",
            Self::UserInactive => "user_inactive",
            Self::EmailMissing => "email_missing",
            Self::EmailInUse => "email_in_use",
            Self::UpstreamDenied => "upstream_denied",
        }
    }
}

/// Why [`crate::sso`] could not complete a step.
#[derive(Debug, Error)]
pub enum SsoError {
    /// The sign-in was understood and **refused** - the user, the token or
    /// the policy is the problem. The browser is told `access_denied`.
    #[error("sign-in refused: {}", .0.reason())]
    Refused(Refusal),
    /// The sign-in could not be evaluated - the `IdP`, the database or the
    /// configuration is the problem. The browser is told `server_error`. The
    /// string is a fixed description; it never contains a secret, a token or
    /// a response body.
    #[error("sign-in unavailable: {0}")]
    Unavailable(String),
}

impl From<Refusal> for SsoError {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}
