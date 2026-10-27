//! The cross-implementation vector: one delivery, sealed with every free choice
//! pinned (the signing seed, the clock and the nonce), written to
//! `tests/fixtures/sealed_delivery_vector.json`.
//!
//! The SDKs hold a byte-identical copy (`cargo xtask sdk-parity-check` compares
//! them) and open it with their own code: the Rust SDK's `WebhookOpener` and the
//! Node SDK's `openWebhook`, which runs the same Rust compiled to WebAssembly.
//! Three implementations of "open a sealed webhook" agreeing about bytes this
//! crate produced is the cross-language proof the HMAC scheme had in its
//! `cross_language_vectors.json`, for the scheme that replaced it.
//!
//! Ed25519 signatures and COSE encoding are deterministic, so the same inputs
//! seal to the same bytes forever; a change to the sealer, the contract or the
//! cratestack pin moves them and fails this test. Regenerate on purpose with
//! `SMS_WEBHOOK_WRITE_VECTORS=1 cargo test -p sms-webhook --test sealed_vector`,
//! then `pnpm exec biome format --write` the file and copy it over
//! `sdks/rust/vsms-sdk-rust/tests/fixtures/` (the Rust and wasm SDKs read it there).

use std::path::PathBuf;
use std::sync::Arc;

use cratestack::cose::{CoseEnvelope, CoseMode, Ed25519Signer, StaticVerifierResolver};
use serde_json::{Value, json};
use sms_webhook::{CONTRACT_SHA_HEX, Sealer};

/// RFC 8032 test vector 1's secret key, the first key the SDKs' tests pin.
const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];
const CTI: [u8; 16] = [0xc7; 16];
const IAT: i64 = 1_793_000_000;
const AUDIENCE: &str = "vaam";
const URL: &str = "https://api.vaam.store/webhooks/vsms";
const ROUTE: &str = "/webhooks/vsms";
const BODY: &str = r#"{"id":"cattempt00000000000000","eventId":"11111111-1111-4111-8111-111111111111","type":"message.delivered","occurredAt":"2026-10-27T10:00:00Z","data":{"messageId":"cmsg000000000000000000","to":"+2376****89","state":"delivered"}}"#;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sealed_delivery_vector.json")
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

async fn sealed_vector() -> Value {
    let signer = Ed25519Signer::from_seed(&SEED);
    let public = signer.verify_key().ed25519_bytes().unwrap();
    let cose = CoseEnvelope::client(
        CoseMode::Sign1,
        Arc::new(signer),
        Arc::new(StaticVerifierResolver::new()),
    )
    .clock(|| IAT)
    .cti_source(|| Ok(CTI.to_vec()))
    .build()
    .unwrap();
    let delivery = Sealer::from_envelope(cose)
        .unwrap()
        .seal(AUDIENCE, URL, BODY.as_bytes())
        .await
        .unwrap();
    json!({
        "$comment": "Written by backends/crates/sms-webhook/tests/sealed_vector.rs; the SDKs hold a byte-identical copy. Do not edit: regenerate with SMS_WEBHOOK_WRITE_VECTORS=1, then biome format and copy it to the SDK.",
        "seedHex": hex(&SEED),
        "publicKeyHex": hex(&public),
        "audience": AUDIENCE,
        "url": URL,
        "route": ROUTE,
        "iat": IAT,
        "ctiHex": hex(&CTI),
        "contractSha256": CONTRACT_SHA_HEX,
        "plaintext": BODY,
        "sealedHex": hex(&delivery.body),
        "headers": delivery.headers.iter().map(|(name, value)| json!([name, value])).collect::<Vec<_>>(),
    })
}

#[tokio::test]
async fn the_committed_vector_is_what_the_sealer_produces() {
    let produced = sealed_vector().await;
    if std::env::var_os("SMS_WEBHOOK_WRITE_VECTORS").is_some() {
        let mut text = serde_json::to_string_pretty(&produced).unwrap();
        text.push('\n');
        std::fs::write(fixture_path(), text).unwrap();
    }
    let committed: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture_path()).unwrap()).unwrap();
    assert_eq!(
        committed, produced,
        "the sealer no longer produces the committed vector: if that is intended, regenerate it \
         (SMS_WEBHOOK_WRITE_VECTORS=1) and let the SDKs' copies follow"
    );
}
