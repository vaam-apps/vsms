#![doc = include_str!("lib.md")]

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// `X-Sms-Event` — the event type, verbatim (e.g. `message.delivered`).
/// Not covered by the signature itself; carried for routing/logging only.
pub const HEADER_EVENT: &str = "X-Sms-Event";

/// `X-Sms-Event-Id` — `WebhookAttempt.sourceEventId`. Part of the signed
/// canonical string, and — per §4.4 — the receiver's documented dedupe
/// key ("Send `X-Sms-Event-Id` and mean it — delivery is at-least-once
/// and receivers need a dedupe key").
pub const HEADER_EVENT_ID: &str = "X-Sms-Event-Id";

/// `X-Sms-Timestamp` — Unix seconds, decimal. Part of the signed canonical
/// string.
pub const HEADER_TIMESTAMP: &str = "X-Sms-Timestamp";

/// `X-Sms-Signature` — `v1=<hex>[,v1=<hex>]`. See [`sign_header`] and
/// [`verify`].
pub const HEADER_SIGNATURE: &str = "X-Sms-Signature";

/// The one signature scheme this crate speaks. A future second scheme
/// would be a *new* version prefix (`v2`), not a change to what `v1`
/// means — folded into the signed bytes themselves (see
/// [`canonical_string`]), not just the header's `v1=` prefix, so that a
/// `v2` value could never be replayed as if it verified under `v1`'s
/// algorithm.
pub const SIGNATURE_VERSION: &str = "v1";

/// 32 bytes — [`generate_secret`]'s output size before hex encoding.
/// Matches `backends/crates/sms-api/src/pepper.rs`'s `MIN_PEPPER_BYTES`: 256 bits,
/// the same floor HMAC-SHA256's own key/output size implies (RFC 2104
/// recommends a key at least as long as the hash output).
const SECRET_BYTES: usize = 32;

/// The prefix every [`generate_secret`] output carries — the
/// Stripe/GitHub convention of a recognisable secret-material prefix, so a
/// human skimming a log, or an automated secret scanner, recognises a
/// leaked value for what it is.
const SECRET_PREFIX: &str = "whsec_";

type HmacSha256 = Hmac<Sha256>;

/// `v1\n{timestamp}\n{eventId}\n{sha256_hex(body)}` — the exact bytes
/// [`sign_v1`]/[`verify`] HMAC. Exposed directly (not folded silently into
/// [`sign_v1`]) for tests, tooling, and anyone implementing a receiver in
/// a language this crate can't help — the module doc's own "Canonical
/// string" section is a transcription of this function, not the other way
/// around.
#[must_use]
pub fn canonical_string(timestamp: i64, event_id: &str, body: &[u8]) -> String {
    let body_hash = hex::encode(Sha256::digest(body));
    format!("{SIGNATURE_VERSION}\n{timestamp}\n{event_id}\n{body_hash}")
}

/// `HMAC-SHA256(secret, canonical_string(timestamp, event_id, body))`,
/// lowercase hex, with no `v1=` prefix — the value that goes after `v1=`
/// in `X-Sms-Signature`. Signing with more than one secret (a rotation
/// overlap window) is [`sign_header`], which reuses the canonical string
/// across every secret rather than recomputing it.
///
/// # Panics
///
/// Never in practice: HMAC (RFC 2104) accepts a key of any length,
/// including the empty string — `Hmac::new_from_slice` only errors for a
/// key length an algorithm structurally can't accept, which SHA-256's
/// HMAC has none of. Same reasoning as
/// `backends/crates/sms-api/src/pepper.rs`'s `hmac_sha256_hex`.
#[must_use]
pub fn sign_v1(secret: &str, timestamp: i64, event_id: &str, body: &[u8]) -> String {
    sign_canonical(secret, &canonical_string(timestamp, event_id, body))
}

/// `HMAC-SHA256(secret, canonical)`, lowercase hex — the shared inner step
/// [`sign_v1`] and [`sign_header`] both call, so the canonical string is
/// computed once per call to either, never once per secret.
fn sign_canonical(secret: &str, canonical: &str) -> String {
    let mut mac = new_mac(secret);
    mac.update(canonical.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Never fails in practice — see [`sign_v1`]'s own `# Panics` section for
/// why `expect` is safe here.
fn new_mac(secret: &str) -> HmacSha256 {
    HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC-SHA256 accepts a key of any length")
}

/// The full `X-Sms-Signature` header value for one or more secrets, in the
/// order given — pass `&[current]` outside a rotation window, or
/// `&[current, prev_secret]` during one (current first, `prevSecret`
/// second — "oldest last" per §4.4). Computes the canonical string once
/// and reuses it across every secret, rather than re-hashing `body` once
/// per secret.
#[must_use]
pub fn sign_header(secrets: &[&str], timestamp: i64, event_id: &str, body: &[u8]) -> String {
    let canonical = canonical_string(timestamp, event_id, body);
    secrets
        .iter()
        .map(|secret| format!("{SIGNATURE_VERSION}={}", sign_canonical(secret, &canonical)))
        .collect::<Vec<_>>()
        .join(",")
}

/// Verifies `signature_header` against every entry in `candidate_secrets`
/// — the intended call shape is current secret first, `prevSecret` second
/// during a rotation window, but this function tries every secret against
/// every presented `v1=` value and accepts on the first match, so the
/// order given doesn't change the result.
///
/// Returns `false` on anything malformed (no recognised `v1=` entry, an
/// entry that isn't valid hex, a hex value of the wrong length, no
/// candidate secrets) rather than an error — a webhook receiver's answer
/// to "is this authentic" only ever needs to be yes or no. Every
/// comparison against a computed HMAC goes through
/// [`hmac::Mac::verify_slice`] (constant-time; see the module doc's own
/// section on this) — never a `==` on hex strings or raw bytes.
#[must_use]
pub fn verify(
    candidate_secrets: &[&str],
    timestamp: i64,
    event_id: &str,
    body: &[u8],
    signature_header: &str,
) -> bool {
    let canonical = canonical_string(timestamp, event_id, body);
    let v1_prefix = format!("{SIGNATURE_VERSION}=");

    let presented_tags: Vec<Vec<u8>> = signature_header
        .split(',')
        .map(str::trim)
        .filter_map(|entry| entry.strip_prefix(v1_prefix.as_str()))
        .filter_map(|hex_value| hex::decode(hex_value).ok())
        .collect();

    if presented_tags.is_empty() {
        return false;
    }

    for secret in candidate_secrets.iter().filter(|secret| !secret.is_empty()) {
        let mut mac = new_mac(secret);
        mac.update(canonical.as_bytes());
        for tag in &presented_tags {
            // `Mac::verify_slice` consumes `self`, so a fresh clone is
            // needed per candidate tag — cheap (it's just the running
            // block-cipher state), and the only way to try more than one
            // tag against the same keyed MAC without recomputing it from
            // the secret each time.
            if mac.clone().verify_slice(tag).is_ok() {
                return true;
            }
        }
    }
    false
}

/// Whether `timestamp` (Unix seconds) is within `tolerance_secs` of `now`
/// (Unix seconds), in either direction — bounds replay of an
/// otherwise-valid signature to a caller-chosen window. Pure: `now` is a
/// parameter, never read from the system clock, so this is deterministic
/// and testable without mocking time.
///
/// Not wired into [`verify`] and not required by §4.4, which specifies the
/// timestamp only as replay-*bounding* material folded into the signed
/// bytes (a tampered timestamp already fails [`verify`]), not as a
/// mandatory freshness check — a receiver decides its own tolerance.
/// `examples/node/webhook-receiver` documents making a different choice
/// (no freshness check at all, relying on `X-Sms-Event-Id` dedupe
/// instead); this function exists for a receiver — or a future `hooks`
/// role check on inbound retries — that wants one. Composable with
/// [`verify`] by any caller that wants both, in either order.
#[must_use]
pub fn is_timestamp_fresh(timestamp: i64, now: i64, tolerance_secs: i64) -> bool {
    now.saturating_sub(timestamp).unsigned_abs() <= tolerance_secs.unsigned_abs()
}

/// A fresh, cryptographically random webhook signing secret:
/// `whsec_<64 lowercase hex chars>` — [`SECRET_BYTES`] bytes of OS
/// randomness (`rand::rngs::OsRng`), hex-encoded, with the [`SECRET_PREFIX`]
/// borrowed from the Stripe/GitHub convention described on that constant.
///
/// The one function in this crate that isn't pure — everything else here
/// is a deterministic function of its arguments. `rotate_webhook_secret`
/// (`backends/crates/sms-api/src/procedures.rs`, #41) is the intended caller.
///
/// # Panics
///
/// Never: `OsRng::fill_bytes` (the infallible `RngCore` method, not the
/// fallible `try_fill_bytes`) is what this calls, and it does not return a
/// `Result`.
#[must_use]
pub fn generate_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; SECRET_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{SECRET_PREFIX}{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independently computed — not with this crate's own code — via:
    ///
    /// ```text
    /// printf '%s' '{"hello":"world"}' | openssl dgst -sha256 -r
    /// # 93a23971a914e5eacbf0a8d25154cda309c3c1c72fbb9914d47c60f3cb681588
    ///
    /// printf 'v1\n1700000000\n11111111-1111-4111-8111-111111111111\n93a23971a914e5eacbf0a8d25154cda309c3c1c72fbb9914d47c60f3cb681588' \
    ///   | openssl dgst -sha256 -hmac "test-secret" -r
    /// # d1af37fc05412e9917cc0418d41500571bdb698cc37205f7786e36267065cb93
    ///
    /// # same canonical string, keyed by "old-secret" instead (the
    /// # rotation-overlap vector below):
    /// printf 'v1\n1700000000\n11111111-1111-4111-8111-111111111111\n93a23971a914e5eacbf0a8d25154cda309c3c1c72fbb9914d47c60f3cb681588' \
    ///   | openssl dgst -sha256 -hmac "old-secret" -r
    /// # 2e284cbe3a6183ac06cd381f56c3f037fecc738b3e8231d0f33f4f6f9d0629e4
    /// ```
    ///
    /// This is what closes the "one genuine guess" gap
    /// `examples/node/webhook-receiver/src/signature.ts` used to carry —
    /// a third, independent implementation (`openssl`, not this crate, not
    /// the Node receiver) landing on the same digest is strong evidence
    /// the algorithm itself, not just this crate's internal consistency,
    /// is correct.
    const VECTOR_TIMESTAMP: i64 = 1_700_000_000;
    const VECTOR_EVENT_ID: &str = "11111111-1111-4111-8111-111111111111";
    const VECTOR_BODY: &[u8] = br#"{"hello":"world"}"#;
    const VECTOR_BODY_SHA256_HEX: &str =
        "93a23971a914e5eacbf0a8d25154cda309c3c1c72fbb9914d47c60f3cb681588";
    const VECTOR_SIG_CURRENT: &str =
        "d1af37fc05412e9917cc0418d41500571bdb698cc37205f7786e36267065cb93";
    const VECTOR_SIG_PREV: &str =
        "2e284cbe3a6183ac06cd381f56c3f037fecc738b3e8231d0f33f4f6f9d0629e4";

    #[test]
    fn canonical_string_matches_the_documented_four_field_shape() {
        let canonical = canonical_string(VECTOR_TIMESTAMP, VECTOR_EVENT_ID, VECTOR_BODY);
        assert_eq!(
            canonical,
            format!("v1\n1700000000\n{VECTOR_EVENT_ID}\n{VECTOR_BODY_SHA256_HEX}")
        );
    }

    #[test]
    fn sign_v1_matches_an_independently_computed_openssl_vector() {
        let sig = sign_v1(
            "test-secret",
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
        );
        assert_eq!(sig, VECTOR_SIG_CURRENT);
    }

    #[test]
    fn sign_header_joins_multiple_secrets_current_first() {
        let header = sign_header(
            &["test-secret", "old-secret"],
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
        );
        assert_eq!(
            header,
            format!("v1={VECTOR_SIG_CURRENT},v1={VECTOR_SIG_PREV}")
        );
    }

    #[test]
    fn verify_accepts_a_signature_made_with_the_current_secret() {
        let header = format!("v1={VECTOR_SIG_CURRENT}");
        assert!(verify(
            &["test-secret"],
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
            &header,
        ));
    }

    #[test]
    fn verify_accepts_a_signature_made_with_prev_secret_during_rotation() {
        // The receiver holds BOTH secrets (current, then prev) during the
        // overlap window; the sender only signed with the old one this
        // time (its "current" secret hasn't rotated on the sender's end
        // in this scenario — see the next test for the header carrying
        // both simultaneously).
        let header = format!("v1={VECTOR_SIG_PREV}");
        assert!(verify(
            &["test-secret", "old-secret"],
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
            &header,
        ));
    }

    #[test]
    fn verify_accepts_either_value_when_the_header_carries_both() {
        let header = sign_header(
            &["test-secret", "old-secret"],
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
        );
        // A receiver holding only the OLD secret (mid-rotation, hasn't
        // learned the new one yet — an unrealistic but instructive edge
        // case) still finds its match among the two presented values.
        assert!(verify(
            &["old-secret"],
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
            &header,
        ));
    }

    #[test]
    fn verify_rejects_a_tampered_body() {
        let header = format!("v1={VECTOR_SIG_CURRENT}");
        assert!(!verify(
            &["test-secret"],
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            br#"{"hello":"world!"}"#,
            &header,
        ));
    }

    #[test]
    fn verify_rejects_a_tampered_timestamp() {
        let header = format!("v1={VECTOR_SIG_CURRENT}");
        assert!(!verify(
            &["test-secret"],
            VECTOR_TIMESTAMP + 1,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
            &header,
        ));
    }

    #[test]
    fn verify_rejects_the_wrong_secret() {
        let header = format!("v1={VECTOR_SIG_CURRENT}");
        assert!(!verify(
            &["some-other-secret"],
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
            &header,
        ));
    }

    #[test]
    fn verify_rejects_a_forged_but_well_formed_signature() {
        let header = format!("v1={}", "0".repeat(64));
        assert!(!verify(
            &["test-secret"],
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
            &header,
        ));
    }

    #[test]
    fn verify_rejects_malformed_headers_without_panicking() {
        for header in ["", "garbage", "v1=", "v1=not-hex", ",", "v2=deadbeef"] {
            assert!(!verify(
                &["test-secret"],
                VECTOR_TIMESTAMP,
                VECTOR_EVENT_ID,
                VECTOR_BODY,
                header,
            ));
        }
    }

    #[test]
    fn verify_rejects_when_no_candidate_secrets_are_given() {
        let header = format!("v1={VECTOR_SIG_CURRENT}");
        assert!(!verify(
            &[],
            VECTOR_TIMESTAMP,
            VECTOR_EVENT_ID,
            VECTOR_BODY,
            &header,
        ));
    }

    #[test]
    fn is_timestamp_fresh_accepts_within_tolerance_in_either_direction() {
        assert!(is_timestamp_fresh(1000, 1000, 0));
        assert!(is_timestamp_fresh(1000, 1300, 300));
        assert!(is_timestamp_fresh(1300, 1000, 300));
        assert!(!is_timestamp_fresh(1000, 1301, 300));
        assert!(!is_timestamp_fresh(1301, 1000, 300));
    }

    #[test]
    fn generate_secret_has_the_documented_shape() {
        let secret = generate_secret();
        assert!(secret.starts_with(SECRET_PREFIX));
        let hex_part = secret.strip_prefix(SECRET_PREFIX).unwrap();
        assert_eq!(hex_part.len(), SECRET_BYTES * 2);
        assert!(
            hex_part
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn generate_secret_is_not_deterministic() {
        assert_ne!(generate_secret(), generate_secret());
    }

    #[test]
    fn sign_v1_is_deterministic_under_the_same_inputs() {
        let a = sign_v1("k", 1, "e", b"body");
        let b = sign_v1("k", 1, "e", b"body");
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_event_id_changes_the_signature() {
        let a = sign_v1("k", 1, "event-a", b"body");
        let b = sign_v1("k", 1, "event-b", b"body");
        assert_ne!(a, b);
    }
}
