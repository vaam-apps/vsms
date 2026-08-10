//! #41's cross-language proof: `tests/fixtures/cross_language_vectors.json`
//! is a shared fixture two *independent* implementations of §4.4's webhook
//! signature scheme are each checked against — this file (Rust, the real
//! sender-side implementation in `crates/sms-webhook/src/lib.rs`) and
//! `examples/node/webhook-receiver/src/signature.ts` (TypeScript, written
//! against §4.4's prose *before* this crate existed — see that file's own
//! module doc for what it calls "the one genuine guess"). Every vector's
//! `signatureHeader` was computed with neither language's code — a third,
//! independent tool (`openssl dgst -sha256 -hmac`, receipts in the fixture
//! file's own `$comment`) — so three implementations landing on the same
//! digest is real evidence the algorithm is unambiguous, not just that two
//! copies of one bug agree.
//!
//! This is a plain, fast, no-`#[ignore]` test — no Docker, no live
//! Postgres, nothing but reading a committed JSON file — so it runs under
//! plain `cargo test --workspace` / `just test`, the same way every other
//! unit-level assertion in this workspace does.
//!
//! Run it directly with:
//!
//! ```bash
//! cargo test -p sms-webhook --test cross_language_fixtures
//! ```

use std::path::Path;

use serde_json::Value;
use sms_webhook::{sign_header, verify};

fn load_vectors() -> Vec<Value> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cross_language_vectors.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let root: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()));
    root["vectors"]
        .as_array()
        .unwrap_or_else(|| panic!("{}: top-level `vectors` must be an array", path.display()))
        .clone()
}

fn str_field<'a>(vector: &'a Value, field: &str, name: &str) -> &'a str {
    vector[field]
        .as_str()
        .unwrap_or_else(|| panic!("vector {name:?} is missing string field {field:?}"))
}

/// This crate's own [`sign_header`] must reproduce every fixture's
/// `signatureHeader` exactly, from the same `secrets`/timestamp/eventId/
/// body — the Rust half of the cross-language proof. A vector whose
/// `secrets` has more than one entry only makes sense for a
/// `signatureHeader` that itself carries every one of those signatures
/// (the `header-carries-both-values-oldest-last` vector); the other
/// multi-secret vectors were each signed with exactly one of their
/// candidate `secrets` — this loop hardcodes which one per vector name,
/// since the fixture format itself has no separate field for it (its
/// `secrets` array is the *receiver's* candidate list, not a record of
/// which one the sender used).
#[test]
fn sms_webhook_reproduces_every_fixture_signature_header() {
    for vector in load_vectors() {
        let name = str_field(&vector, "name", "<unnamed>").to_owned();

        // Only the two vectors that are actually *about* rotation sign
        // with more than one secret at once; every other vector's
        // `signatureHeader` was produced by exactly one of the candidate
        // `secrets` (or, for the negative vectors, by neither — those are
        // skipped here and covered by `sms_webhook_verify_matches_every_fixture_expectation`
        // instead, since there's nothing for `sign_header` to reproduce).
        let signing_secrets: &[&str] = match name.as_str() {
            "signed-with-current-secret" => &["current-secret-abc123"],
            "signed-with-prev-secret-during-rotation" => &["previous-secret-xyz789"],
            "header-carries-both-values-oldest-last" => {
                &["current-secret-abc123", "previous-secret-xyz789"]
            }
            _ => continue,
        };

        let timestamp = vector["timestampUnix"]
            .as_i64()
            .unwrap_or_else(|| panic!("vector {name:?} is missing timestampUnix"));
        let event_id = str_field(&vector, "eventId", &name);
        let body = str_field(&vector, "bodyUtf8", &name);
        let expected_header = str_field(&vector, "signatureHeader", &name);

        let computed = sign_header(signing_secrets, timestamp, event_id, body.as_bytes());
        assert_eq!(
            computed, expected_header,
            "vector {name:?}: sms_webhook::sign_header did not reproduce the fixture's signatureHeader"
        );
    }
}

/// This crate's own [`verify`] must agree with every fixture's
/// `expectVerifies`, given that vector's `secrets` as the candidate list —
/// covers both the positive vectors above and the negative ones
/// (tampered body, wrong secret, malformed header) that
/// `sign_header` alone can't exercise.
#[test]
fn sms_webhook_verify_matches_every_fixture_expectation() {
    for vector in load_vectors() {
        let name = str_field(&vector, "name", "<unnamed>").to_owned();
        let secrets: Vec<&str> = vector["secrets"]
            .as_array()
            .unwrap_or_else(|| panic!("vector {name:?} is missing a `secrets` array"))
            .iter()
            .map(|s| {
                s.as_str().unwrap_or_else(|| {
                    panic!("vector {name:?}: a `secrets` entry was not a string")
                })
            })
            .collect();
        let timestamp = vector["timestampUnix"]
            .as_i64()
            .unwrap_or_else(|| panic!("vector {name:?} is missing timestampUnix"));
        let event_id = str_field(&vector, "eventId", &name);
        let body = str_field(&vector, "bodyUtf8", &name);
        let signature_header = str_field(&vector, "signatureHeader", &name);
        let expect_verifies = vector["expectVerifies"]
            .as_bool()
            .unwrap_or_else(|| panic!("vector {name:?} is missing expectVerifies"));

        let outcome = verify(
            &secrets,
            timestamp,
            event_id,
            body.as_bytes(),
            signature_header,
        );
        assert_eq!(
            outcome, expect_verifies,
            "vector {name:?}: sms_webhook::verify returned {outcome}, fixture expects {expect_verifies}"
        );
    }
}

/// A guard against the fixture file itself silently losing coverage —
/// e.g. someone deletes a vector while debugging and forgets to restore
/// it. Names must match exactly what the two tests above (and the Node
/// side) expect to find.
#[test]
fn the_fixture_file_still_has_every_expected_vector() {
    let names: Vec<String> = load_vectors()
        .iter()
        .map(|v| str_field(v, "name", "<unnamed>").to_owned())
        .collect();
    for expected in [
        "signed-with-current-secret",
        "signed-with-prev-secret-during-rotation",
        "header-carries-both-values-oldest-last",
        "tampered-body-fails",
        "wrong-secret-fails",
        "malformed-signature-header-fails",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "expected vector {expected:?} is missing from the fixture file; names present: {names:?}"
        );
    }
}
