//! The server-held secret key behind `Message.msisdnHash`/`Message.bodyHash`
//! (#134).
//!
//! `docs/architecture.md` has always *described* these columns as HMAC-SHA256
//! under a pepper. Until #134 the implementation was plain, unkeyed
//! `SHA-256` — reversible in seconds over Cameroon's ~10^7-candidate mobile
//! numbering space, which means a "purge" that clears `msisdn` but keeps
//! `msisdnHash` had not de-identified anything. This module is the fix: a
//! [`HashPepper`] newtype carrying real secret material (never the
//! database — `@sensitive`/`@pii` redact audit snapshots only, per
//! `AGENTS.md` §2.0, so a schema field could never have been a
//! confidentiality control here), and [`hmac_sha256_hex`], the one place
//! that turns a pepper + plaintext into the stored form.
//!
//! # Stored form
//!
//! `"{HASH_SCHEME}:{hex}"` — e.g. `hmac-sha256-v1:9f86d0...`. The old
//! `sha256:` prefix was, by its own doc comment, written specifically so a
//! future keyed scheme would be distinguishable per row; this is that
//! migration. The `-v1` suffix is deliberate, not decoration: a future
//! pepper *rotation* needs its own scheme tag (`hmac-sha256-v2:`, keyed
//! under a new pepper) so old and new rows are distinguishable by the
//! stored value alone, the same reasoning the original prefix existed for.
//!
//! # No dual-read / rehash path exists, and none is being added
//!
//! Per `AGENTS.md`, there is no live database anywhere in this deployment
//! yet. That makes this a clean cutover: every `sha256:`-prefixed value in
//! this tree is test fixture data, not production data, so there is
//! nothing to migrate and no dual-read path is worth building. If this
//! lands after real traffic exists, that assumption no longer holds — see
//! the rotation consequence below, which is the same problem in miniature.
//!
//! # Rotation consequence (documented, not solved here)
//!
//! Rotating the pepper — deploying a new [`HashPepper`] value — changes
//! every hash this process computes from that moment on, but does **not**
//! retroactively rehash a single already-stored row. The consequence is
//! asymmetric and worth spelling out precisely:
//!
//! - A `Message`/`OptOut` row that still holds plaintext `msisdn` *can* be
//!   rehashed under the new pepper by a batch job that reads the plaintext
//!   and rewrites `msisdnHash` — no design for that job exists yet.
//! - A row whose `msisdn` has already been purged (the entire point of
//!   `msisdnHash` existing) cannot: there is no plaintext left to rehash
//!   from, so that row's hash is permanently stuck under the old pepper.
//! - Until any such rehashing happens, `OptOut` matching and dedupe against
//!   old rows silently stop working the instant the pepper rotates — a
//!   `msisdnHash` computed under the new pepper will never equal one stored
//!   under the old one, and nothing here detects the mismatch; it just
//!   looks like "not opted out" or "not a duplicate" to the day-one code
//!   that only ever compares hashes computed under whatever pepper is
//!   currently configured.
//!
//! This is the same shape of tradeoff `ProviderError::Indeterminate`
//! documents elsewhere in this codebase: a real, accepted operational
//! consequence, written down rather than hidden behind a migration nobody
//! has designed yet.

use std::fmt;

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Minimum accepted pepper length, in bytes.
///
/// 32 bytes (256 bits) matches `HMAC-SHA256`'s own key/output size — a
/// shorter key is weaker than the digest it protects, which defeats the
/// point of keying the hash at all (RFC 2104 recommends a key at least as
/// long as the hash output). This is a floor, not a target: operators
/// should generate this with something like `openssl rand -base64 48`, not
/// hand-type a short phrase that happens to clear the check.
pub const MIN_PEPPER_BYTES: usize = 32;

/// The scheme tag `hmac_sha256_hex` stamps onto every hash it produces.
/// Distinguishes this scheme from the pre-#134 unkeyed `sha256:` values
/// still present in test fixtures, and from any future `-v2` scheme a
/// pepper rotation introduces.
pub const HASH_SCHEME: &str = "hmac-sha256-v1";

type HmacSha256 = Hmac<Sha256>;

/// Real secret material — a server-held pepper, never present in the
/// database, a migration, or a log line.
///
/// Deliberately not `#[derive(Debug)]`: [`fmt::Debug`] is hand-written below
/// to redact the value unconditionally, so embedding this in a larger
/// struct that *does* derive `Debug` (e.g. a future config struct) cannot
/// accidentally print the pepper — the redaction travels with the type,
/// not with the discipline of whoever holds it.
#[derive(Clone)]
pub struct HashPepper(std::sync::Arc<str>);

impl HashPepper {
    /// Wrap `value` as a pepper, rejecting anything shorter than
    /// [`MIN_PEPPER_BYTES`].
    ///
    /// # Errors
    ///
    /// [`PepperError::TooShort`] if `value` is shorter than
    /// [`MIN_PEPPER_BYTES`] bytes. Callers (`sms-gateway serve`'s CLI
    /// parsing) are expected to treat this as fatal at startup — failing
    /// loudly before the first request, not the first send.
    pub fn new(value: impl Into<String>) -> Result<Self, PepperError> {
        let value = value.into();
        if value.len() < MIN_PEPPER_BYTES {
            return Err(PepperError::TooShort {
                min: MIN_PEPPER_BYTES,
                actual: value.len(),
            });
        }
        Ok(Self(std::sync::Arc::from(value)))
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for HashPepper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("HashPepper").field(&"<redacted>").finish()
    }
}

/// Why a candidate pepper was refused.
#[derive(Debug, thiserror::Error)]
pub enum PepperError {
    /// Shorter than [`MIN_PEPPER_BYTES`] — see [`HashPepper::new`].
    #[error(
        "hash pepper is too short: got {actual} byte(s), need at least {min} — \
         generate one with e.g. `openssl rand -base64 48`"
    )]
    TooShort {
        /// [`MIN_PEPPER_BYTES`].
        min: usize,
        /// The rejected value's length, in bytes.
        actual: usize,
    },
}

/// `{HASH_SCHEME}:{hex(HMAC-SHA256(pepper, input))}` — the one place this
/// crate turns a pepper and a plaintext into a stored hash.
///
/// Exposed as a free function (not a private method on `Procedures`) so
/// live-Postgres tests can compute the exact value the send path will
/// persist for a given pepper/input pair, rather than hand-rolling a
/// second copy of the algorithm that can silently drift from this one —
/// which is exactly the shape of the pre-#134 `sha256:` convention's own
/// test helper (`send_message_live_postgres.rs`'s `sha_of`), and exactly
/// the risk this indirection removes.
///
/// # Panics
///
/// Never in practice. `Hmac::new_from_slice` only returns an error for a
/// key length an algorithm can't accept at all; HMAC (RFC 2104) accepts
/// any key length, including ones longer than the block size (they get
/// hashed down first). `HashPepper` enforces a minimum length, never a
/// maximum, so no value it can hold ever reaches this `expect`.
#[must_use]
pub fn hmac_sha256_hex(pepper: &HashPepper, input: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(pepper.as_bytes()).expect("HMAC-SHA256 accepts any key length");
    mac.update(input.as_bytes());
    let digest = mac.finalize().into_bytes();

    let mut hex = String::with_capacity(HASH_SCHEME.len() + 1 + digest.len() * 2);
    hex.push_str(HASH_SCHEME);
    hex.push(':');
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pepper_shorter_than_the_minimum_is_rejected() {
        let error = HashPepper::new("short").unwrap_err();
        assert!(matches!(error, PepperError::TooShort { .. }));
    }

    #[test]
    fn a_pepper_at_exactly_the_minimum_is_accepted() {
        let value = "x".repeat(MIN_PEPPER_BYTES);
        assert!(HashPepper::new(value).is_ok());
    }

    #[test]
    fn debug_never_prints_the_pepper() {
        let pepper = HashPepper::new("a".repeat(MIN_PEPPER_BYTES)).unwrap();
        let printed = format!("{pepper:?}");
        assert!(!printed.contains(&"a".repeat(MIN_PEPPER_BYTES)));
        assert!(printed.contains("redacted"));
    }

    #[test]
    fn the_hash_is_stable_prefixed_and_keyed() {
        let pepper_a = HashPepper::new("a".repeat(MIN_PEPPER_BYTES)).unwrap();
        let pepper_b = HashPepper::new("b".repeat(MIN_PEPPER_BYTES)).unwrap();

        let hash = hmac_sha256_hex(&pepper_a, "+237677123456");
        assert!(hash.starts_with("hmac-sha256-v1:"));
        assert_eq!(hash.len(), "hmac-sha256-v1:".len() + 64);
        assert_eq!(
            hash,
            hmac_sha256_hex(&pepper_a, "+237677123456"),
            "must be deterministic under the same pepper"
        );
        assert_ne!(
            hash,
            hmac_sha256_hex(&pepper_a, "+237677123457"),
            "must not collide trivially"
        );
        assert_ne!(
            hash,
            hmac_sha256_hex(&pepper_b, "+237677123456"),
            "the same plaintext under a different pepper must hash differently — \
             this is the entire point of keying it"
        );
    }
}
