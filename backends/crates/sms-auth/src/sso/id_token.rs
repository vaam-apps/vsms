//! Verifying the upstream `id_token`. Pure: no I/O, no clock but the
//! system's, no state - every refusal here is unit-testable with a token
//! built in the test.
//!
//! What is checked, and in what order (each a hard stop):
//!
//! 1. The header's `alg` is on the allow-list. `none` cannot even be parsed
//!    into an [`Algorithm`] (so it fails here too), the `HS*` family and
//!    `EdDSA` are not on the list - a symmetric algorithm keyed with a value
//!    the attacker can read (the public key) is the classic confusion
//!    attack, and an OIDC relying party that accepts the `IdP`'s public key as
//!    an HMAC secret has accepted forgery.
//! 2. The header has a `kid` (selection of the key is the caller's job; this
//!    function is handed the one key that `kid` named).
//! 3. That key can have signed with that `alg`: same family, `use` is `sig`
//!    or absent, and a stated `alg` equals the header's.
//! 4. The signature, and `exp`/`nbf` with a 60-second leeway, `iss`
//!    byte-for-byte, `aud` containing the client id (jsonwebtoken).
//! 5. `iat` present and not in the future beyond the leeway; `sub` present,
//!    non-empty, at most 255 characters (OIDC Core 2.); `azp` equal to the
//!    client id when present, and *required* when `aud` has several values.
//! 6. `nonce` equal to this transaction's, in constant time.

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, get_current_timestamp};
use serde_json::Value;
use subtle::ConstantTimeEq;

use super::error::Refusal;

/// Leeway for `exp`, `nbf` and `iat`, in seconds.
pub const LEEWAY_SECS: u64 = 60;

/// OIDC Core 2: `sub` is at most 255 ASCII characters.
const MAX_SUBJECT_LEN: usize = 255;

/// What the caller knows the token must say.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    /// The configured issuer, compared byte for byte.
    pub issuer: &'a str,
    /// This gateway's client id.
    pub client_id: &'a str,
    /// The `nonce` this transaction sent upstream.
    pub nonce: &'a str,
    /// The claim the group list is read from.
    pub groups_claim: &'a str,
}

/// The claims of a verified `id_token` that the rest of the broker uses.
/// `email`, `name` and `preferred_username` are **display data** - they
/// never select an account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdClaims {
    /// `sub`, unique within the issuer.
    pub subject: String,
    /// `email`.
    pub email: Option<String>,
    /// `email_verified`: `Some(true)` only for the JSON boolean `true`. A
    /// string `"true"`, a number, or no claim at all is `None` - the identity
    /// provider has not said the address is its user's, and an account is
    /// never created on an address nobody confirmed.
    pub email_verified: Option<bool>,
    /// `name`.
    pub name: Option<String>,
    /// `preferred_username`.
    pub preferred_username: Option<String>,
    /// `acr`.
    pub acr: Option<String>,
    /// `amr`.
    pub amr: Vec<String>,
    /// The members of the configured groups claim.
    pub groups: Vec<String>,
}

/// The JOSE names of the allowed algorithms, for checking a header before it
/// is parsed into an [`Algorithm`].
pub const ALLOWED_ALGORITHM_NAMES: &[&str] = &[
    "RS256", "RS384", "RS512", "PS256", "PS384", "PS512", "ES256", "ES384",
];

/// The `alg` string in a compact JWS's protected header, read without
/// verifying anything.
#[must_use]
pub fn peek_algorithm_name(raw: &str) -> Option<String> {
    use base64::Engine as _;
    let header = raw.split('.').next()?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(header)
        .ok()?;
    let header: Value = serde_json::from_slice(&bytes).ok()?;
    header.get("alg").and_then(Value::as_str).map(str::to_owned)
}

/// Whether `alg` may sign an `id_token` this broker accepts: RSA (PKCS#1 v1.5
/// and PSS) and ECDSA only.
#[must_use]
pub fn is_allowed_algorithm(alg: Algorithm) -> bool {
    matches!(
        alg,
        Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512
            | Algorithm::ES256
            | Algorithm::ES384
    )
}

/// Whether `jwk` can have signed a token whose header says `alg`.
///
/// # Errors
///
/// [`Refusal::KeyUnsuitable`] when the key family, `use`, or stated `alg`
/// disagree.
pub fn check_key_suits(jwk: &Jwk, alg: Algorithm) -> Result<(), Refusal> {
    let family_ok = matches!(
        (&jwk.algorithm, alg),
        (
            AlgorithmParameters::RSA(_),
            Algorithm::RS256
                | Algorithm::RS384
                | Algorithm::RS512
                | Algorithm::PS256
                | Algorithm::PS384
                | Algorithm::PS512
        ) | (
            AlgorithmParameters::EllipticCurve(_),
            Algorithm::ES256 | Algorithm::ES384
        )
    );
    if !family_ok {
        return Err(Refusal::KeyUnsuitable);
    }
    if let Some(usage) = &jwk.common.public_key_use
        && *usage != PublicKeyUse::Signature
    {
        return Err(Refusal::KeyUnsuitable);
    }
    if let Some(stated) = jwk.common.key_algorithm
        && stated.to_string() != format!("{alg:?}")
    {
        return Err(Refusal::KeyUnsuitable);
    }
    Ok(())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn optional_string(claims: &Value, key: &str) -> Option<String> {
    claims.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn map_jwt_error(kind: &ErrorKind) -> Refusal {
    match kind {
        ErrorKind::InvalidSignature => Refusal::SignatureInvalid,
        ErrorKind::ExpiredSignature => Refusal::Expired,
        ErrorKind::ImmatureSignature => Refusal::NotYetValid,
        ErrorKind::InvalidIssuer => Refusal::IssuerMismatch,
        ErrorKind::InvalidAudience => Refusal::AudienceMismatch,
        ErrorKind::MissingRequiredClaim(_) | ErrorKind::InvalidSubject => Refusal::MissingClaim,
        ErrorKind::InvalidAlgorithm | ErrorKind::MissingAlgorithm => Refusal::AlgorithmNotAllowed,
        _ => Refusal::IdTokenMalformed,
    }
}

/// Verifies `raw` against `jwk` (already selected by the header's `kid`) and
/// `expected`.
///
/// # Errors
///
/// A [`Refusal`] naming the first check that failed.
pub fn verify(
    raw: &str,
    jwk: &Jwk,
    alg: Algorithm,
    expected: &Expected<'_>,
) -> Result<IdClaims, Refusal> {
    if !is_allowed_algorithm(alg) {
        return Err(Refusal::AlgorithmNotAllowed);
    }
    check_key_suits(jwk, alg)?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| Refusal::KeyUnsuitable)?;

    let mut validation = Validation::new(alg);
    validation.leeway = LEEWAY_SECS;
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.validate_aud = true;
    validation.set_audience(&[expected.client_id]);
    validation.set_issuer(&[expected.issuer]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

    let data =
        decode::<Value>(raw, &key, &validation).map_err(|error| map_jwt_error(error.kind()))?;
    let claims = data.claims;

    // `sub`.
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.is_empty() && sub.len() <= MAX_SUBJECT_LEN)
        .ok_or(Refusal::MissingClaim)?
        .to_owned();

    // `iat`: required, and not from the future.
    let iat = claims
        .get("iat")
        .and_then(Value::as_u64)
        .ok_or(Refusal::MissingClaim)?;
    if iat > get_current_timestamp().saturating_add(LEEWAY_SECS) {
        return Err(Refusal::IssuedInFuture);
    }

    // `azp`: equal when present; required when `aud` names several parties.
    let several_audiences = claims
        .get("aud")
        .and_then(Value::as_array)
        .is_some_and(|aud| aud.len() > 1);
    match claims.get("azp") {
        Some(Value::String(azp)) if constant_time_eq(azp, expected.client_id) => {}
        Some(_) => return Err(Refusal::AuthorizedPartyMismatch),
        None if several_audiences => return Err(Refusal::AuthorizedPartyMismatch),
        None => {}
    }

    // `nonce`.
    match claims.get("nonce").and_then(Value::as_str) {
        Some(nonce) if !nonce.is_empty() && constant_time_eq(nonce, expected.nonce) => {}
        _ => return Err(Refusal::NonceMismatch),
    }

    Ok(IdClaims {
        subject,
        email: optional_string(&claims, "email"),
        email_verified: claims.get("email_verified").and_then(Value::as_bool),
        name: optional_string(&claims, "name"),
        preferred_username: optional_string(&claims, "preferred_username"),
        acr: optional_string(&claims, "acr"),
        amr: string_list(claims.get("amr")),
        groups: string_list(claims.get(expected.groups_claim)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_asymmetric_rsa_and_ecdsa_algorithms_are_allowed() {
        for alg in [
            Algorithm::RS256,
            Algorithm::PS256,
            Algorithm::ES256,
            Algorithm::ES384,
        ] {
            assert!(is_allowed_algorithm(alg), "{alg:?}");
        }
        for alg in [
            Algorithm::HS256,
            Algorithm::HS384,
            Algorithm::HS512,
            Algorithm::EdDSA,
        ] {
            assert!(!is_allowed_algorithm(alg), "{alg:?}");
        }
    }

    #[test]
    fn list_claims_accept_a_string_or_an_array_and_ignore_other_shapes() {
        assert_eq!(string_list(Some(&serde_json::json!("a"))), vec!["a"]);
        assert_eq!(
            string_list(Some(&serde_json::json!(["a", 1, "b"]))),
            vec!["a", "b"]
        );
        assert_eq!(
            string_list(Some(&serde_json::json!({"a": 1}))),
            Vec::<String>::new()
        );
        assert_eq!(string_list(None), Vec::<String>::new());
    }

    #[test]
    fn constant_time_eq_is_plain_equality_semantically() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
