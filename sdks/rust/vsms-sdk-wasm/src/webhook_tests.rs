//! Native tests of [`crate::webhook::WebhookOpener`], against deliveries sealed
//! the way the gateway's worker seals them (cratestack's `ClientEnvelope`), and
//! against the vector the gateway's own `sms-webhook` crate wrote.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cratestack::client_rust::{ClientEnvelope, RouteRef, SealCall};
use cratestack::cose::{CoseEnvelope, CoseMode, Ed25519Signer, StaticVerifierResolver};
use cratestack::serde_json;

use crate::core::ServerKeys;
use crate::webhook::{webhook_contract_sha, WebhookError, WebhookOpener};

const GATEWAY_SEED: [u8; 32] = [21; 32];
const NEXT_SEED: [u8; 32] = [22; 32];
const STRANGER_SEED: [u8; 32] = [23; 32];
const AUDIENCE: &str = "vaam";
const ROUTE: &str = "/webhooks/vsms";
const BODY: &str = r#"{"id":"a","eventId":"11111111-1111-4111-8111-111111111111","type":"message.delivered","occurredAt":"2026-10-27T10:00:00Z","data":{"state":"delivered"}}"#;

fn public(seed: &[u8; 32]) -> [u8; 32] {
    Ed25519Signer::from_seed(seed)
        .verify_key()
        .ed25519_bytes()
        .unwrap()
}

fn keys() -> ServerKeys {
    ServerKeys::new(public(&GATEWAY_SEED), public(&NEXT_SEED)).unwrap()
}

fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

struct Delivery {
    seed: [u8; 32],
    audience: &'static str,
    route: &'static str,
    contract: [u8; 32],
    iat: i64,
}

impl Delivery {
    fn genuine() -> Self {
        Self {
            seed: GATEWAY_SEED,
            audience: AUDIENCE,
            route: ROUTE,
            contract: webhook_contract_sha(),
            iat: now(),
        }
    }

    async fn seal(&self) -> Vec<u8> {
        let iat = self.iat;
        let cose = CoseEnvelope::client(
            CoseMode::Sign1,
            Arc::new(Ed25519Signer::from_seed(&self.seed)),
            Arc::new(StaticVerifierResolver::new()),
        )
        .clock(move || iat)
        .build()
        .unwrap();
        let envelope = ClientEnvelope::new(cose, self.audience).unwrap();
        let call = SealCall::new("POST", RouteRef::new(self.route, &[]), self.contract)
            .payload(BODY.as_bytes(), "application/json")
            .accept("application/json");
        envelope.seal_call(call).await.unwrap().body.to_vec()
    }
}

fn opener() -> WebhookOpener {
    WebhookOpener::new(&keys(), AUDIENCE, ROUTE, None).unwrap()
}

#[tokio::test]
async fn a_genuine_delivery_opens_to_the_envelope_text() {
    let text = opener()
        .open(&Delivery::genuine().seal().await)
        .await
        .unwrap();
    assert_eq!(text, BODY);
}

#[tokio::test]
async fn every_wrong_delivery_is_the_one_coarse_refusal() {
    let opener = opener();
    let wrong = [
        Delivery {
            audience: "someone-else",
            ..Delivery::genuine()
        },
        Delivery {
            route: "/webhooks/other",
            ..Delivery::genuine()
        },
        Delivery {
            seed: STRANGER_SEED,
            ..Delivery::genuine()
        },
        Delivery {
            contract: [9; 32],
            ..Delivery::genuine()
        },
        Delivery {
            iat: now() - 3_600,
            ..Delivery::genuine()
        },
    ];
    for delivery in wrong {
        assert_eq!(
            opener.open(&delivery.seal().await).await,
            Err(WebhookError::Unverified)
        );
    }
    let mut tampered = Delivery::genuine().seal().await;
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert_eq!(opener.open(&tampered).await, Err(WebhookError::Unverified));
    assert_eq!(opener.open(b"nope").await, Err(WebhookError::Unverified));
}

#[tokio::test]
async fn a_replay_is_refused_and_a_retry_is_not() {
    let opener = opener();
    let first = Delivery::genuine().seal().await;
    assert!(opener.open(&first).await.is_ok());
    assert_eq!(opener.open(&first).await, Err(WebhookError::Unverified));
    assert!(opener.open(&Delivery::genuine().seal().await).await.is_ok());
}

#[tokio::test]
async fn the_next_key_opens_too() {
    let delivery = Delivery {
        seed: NEXT_SEED,
        ..Delivery::genuine()
    };
    assert!(opener().open(&delivery.seal().await).await.is_ok());
}

#[test]
fn a_misconfigured_opener_is_refused_up_front() {
    assert!(matches!(
        WebhookOpener::new(&keys(), "", ROUTE, None),
        Err(WebhookError::Invalid(_))
    ));
    assert!(matches!(
        WebhookOpener::new(&keys(), AUDIENCE, "no-slash", None),
        Err(WebhookError::Invalid(_))
    ));
}

/// The bytes `sms-webhook`'s own sealer wrote (its `tests/sealed_vector.rs`),
/// opened by the code the Node SDK runs.
#[tokio::test]
async fn the_gateways_own_vector_opens() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../vsms-sdk-rust/tests/fixtures/sealed_delivery_vector.json"
    ))
    .unwrap();
    let vector: serde_json::Value = serde_json::from_str(&text).unwrap();
    let hex = |name: &str| -> Vec<u8> {
        let text = vector[name].as_str().unwrap();
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    };
    let public_key: [u8; 32] = hex("publicKeyHex").try_into().unwrap();
    let keys = ServerKeys::new(public_key, public(&NEXT_SEED)).unwrap();
    let iat = vector["iat"].as_i64().unwrap();
    let opener = WebhookOpener::new(
        &keys,
        vector["audience"].as_str().unwrap(),
        vector["route"].as_str().unwrap(),
        Some(Arc::new(move || iat)),
    )
    .unwrap();
    let opened = opener.open(&hex("sealedHex")).await.unwrap();
    assert_eq!(opened, vector["plaintext"].as_str().unwrap());
}
