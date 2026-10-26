//! The server's pinned public keys.
//!
//! The gateway signs every answer with an Ed25519 key, and holds a second one
//! for the next rotation. A client pins **both**, so a rotation is: ship
//! clients that pin the new key as `next`, promote it on the server, retire the
//! old one. No flag day, and an answer signed by neither key never verifies.
//!
//! The values come from `sms-gateway server-keys`, which prints
//!
//! ```text
//! current: <base64url>
//! next: <base64url>
//! ```
//!
//! and are public: put them in configuration, not in a secret store.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use cratestack::cose::CoseVerifyKey;

use crate::error::SdkError;

/// The `{current, next}` public keys of one gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerKeys {
    current: [u8; 32],
    next: [u8; 32],
}

fn decode(label: &str, text: &str) -> Result<[u8; 32], SdkError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(text.trim())
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| {
            SdkError::Config(format!(
                "the {label} server key must be 32 bytes as unpadded base64url"
            ))
        })?;
    CoseVerifyKey::ed25519(&bytes)
        .map_err(|_| SdkError::Config(format!("the {label} server key is not an Ed25519 point")))?;
    Ok(bytes)
}

impl ServerKeys {
    /// The keys, each as unpadded base64url.
    ///
    /// ```
    /// use vsms_sdk::ServerKeys;
    ///
    /// // RFC 8032 test vectors 1 and 2: two distinct, valid Ed25519 points.
    /// let keys = ServerKeys::from_base64url(
    ///     "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
    ///     "PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw",
    /// )
    /// .unwrap();
    /// assert_ne!(keys.current(), keys.next());
    /// // The same key twice would make a rotation unobservable, so it is refused.
    /// assert!(ServerKeys::from_base64url(
    ///     "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
    ///     "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
    /// )
    /// .is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// [`SdkError::Config`] for a value that is not 32 bytes of unpadded
    /// base64url, is not an Ed25519 point, or when both keys are the same.
    pub fn from_base64url(current: &str, next: &str) -> Result<Self, SdkError> {
        let (current, next) = (decode("current", current)?, decode("next", next)?);
        if current == next {
            return Err(SdkError::Config(
                "the current and next server keys are the same key".to_owned(),
            ));
        }
        Ok(Self { current, next })
    }

    /// Parses `sms-gateway server-keys`' output (`current: …` / `next: …`).
    ///
    /// ```
    /// use vsms_sdk::ServerKeys;
    ///
    /// let report = "current: 11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo\nnext: PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw\n";
    /// assert!(ServerKeys::parse_report(report).is_ok());
    /// assert!(ServerKeys::parse_report("current: only-one\n").is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// [`SdkError::Config`] when a line is missing or a key is invalid.
    pub fn parse_report(report: &str) -> Result<Self, SdkError> {
        let field = |label: &str| {
            report
                .lines()
                .find_map(|line| line.trim().strip_prefix(label)?.strip_prefix(':'))
                .map(str::trim)
                .ok_or_else(|| SdkError::Config(format!("no `{label}:` line in the server keys")))
        };
        Self::from_base64url(field("current")?, field("next")?)
    }

    /// The key the server signs with today.
    #[must_use]
    pub fn current(&self) -> &[u8; 32] {
        &self.current
    }

    /// The key the next rotation promotes.
    #[must_use]
    pub fn next(&self) -> &[u8; 32] {
        &self.next
    }
}
