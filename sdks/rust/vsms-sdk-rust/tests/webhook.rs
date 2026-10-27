//! Opening a sealed webhook (`WebhookOpener`): what it accepts, and each way it
//! refuses.
//!
//! The deliveries here are sealed by this file with cratestack's own
//! `ClientEnvelope::seal_call`, the call the gateway's worker makes, and the
//! committed vector (`tests/fixtures/sealed_delivery_vector.json`) is bytes the
//! gateway's own `sms-webhook` crate produced, so the opener is checked against
//! the real sender and not only against itself. Plain `cargo test`: no stack.

use std::sync::Arc;
use std::time::Duration;

use cratestack::client_rust::{ClientEnvelope, RouteRef, SealCall};
use cratestack::cose::{CoseEnvelope, CoseMode, Ed25519Signer, StaticVerifierResolver};
use cratestack::{CratestackError, NonceStore};
use vsms_sdk::{
    webhook_contract_sha, ServerKeys, WebhookError, WebhookOpener, WEBHOOK_CONTRACT_DOCUMENT,
    WEBHOOK_CONTRACT_SHA_HEX,
};

/// RFC 8032 test vector 1's secret key; its public key is the first one `keys()`
/// pins, so a delivery sealed with it is the genuine gateway's for these tests.
const GATEWAY_SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];
const OTHER_SEED: [u8; 32] = [0x13; 32];
const AUDIENCE: &str = "vaam";
const ROUTE: &str = "/webhooks/vsms";
const APP_ID: &str = "capp00000000000000000";
const ENDPOINT_ID: &str = "cendpoint0000000000000";
const BODY: &str = r#"{"id":"cattempt00000000000000","eventId":"11111111-1111-4111-8111-111111111111","type":"message.delivered","occurredAt":"2026-10-27T10:00:00Z","appId":"capp00000000000000000","endpointId":"cendpoint0000000000000","data":{"messageId":"cmsg000000000000000000","state":"delivered"}}"#;
/// The same delivery made for another app's endpoint, which happens to name the
/// same audience.
const OTHER_APP_BODY: &str = r#"{"id":"cattempt00000000000000","eventId":"11111111-1111-4111-8111-111111111111","type":"message.delivered","occurredAt":"2026-10-27T10:00:00Z","appId":"cappother000000000000","endpointId":"cendpoint0000000000000","data":{"messageId":"cmsg000000000000000000","state":"delivered"}}"#;

fn keys() -> ServerKeys {
    ServerKeys::from_base64url(
        "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
        "PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw",
    )
    .expect("two valid public keys")
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

/// A delivery as the gateway's worker seals it: audience, `POST`, the route,
/// the delivery contract, JSON in and out.
struct Delivery<'a> {
    seed: [u8; 32],
    audience: &'a str,
    route: &'a str,
    contract: [u8; 32],
    payload_type: &'a str,
    body: &'a str,
    iat: i64,
}

impl Delivery<'_> {
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
        let envelope = ClientEnvelope::new(cose, self.audience.to_owned()).unwrap();
        let call = SealCall::new("POST", RouteRef::new(self.route, &[]), self.contract)
            .payload(self.body.as_bytes(), self.payload_type)
            .accept("application/json");
        envelope.seal_call(call).await.unwrap().body.to_vec()
    }
}

fn genuine<'a>() -> Delivery<'a> {
    Delivery {
        seed: GATEWAY_SEED,
        audience: AUDIENCE,
        route: ROUTE,
        contract: webhook_contract_sha(),
        payload_type: "application/json",
        body: BODY,
        iat: now(),
    }
}

fn opener() -> WebhookOpener {
    WebhookOpener::new(&keys(), AUDIENCE, ROUTE, APP_ID).unwrap()
}

#[test]
fn the_contract_constants_are_the_documented_ones() {
    let hex: String = webhook_contract_sha()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(hex, WEBHOOK_CONTRACT_SHA_HEX);
    // Independently computed with Python's hashlib:
    //   sha256(b"vsms/webhook-delivery/v2\0" + document)
    assert_eq!(
        WEBHOOK_CONTRACT_SHA_HEX,
        "c61394a50e9f02ece411674fb3f398ae323e685b7a14f9c7b85538aa33a30423"
    );
    assert!(WEBHOOK_CONTRACT_DOCUMENT.contains("\"vsms.webhook.delivery\""));
}

#[tokio::test]
async fn a_genuine_delivery_opens_into_the_typed_event() {
    let event = opener().open(&genuine().seal().await).await.unwrap();
    assert_eq!(event.id, "cattempt00000000000000");
    assert_eq!(event.event_id, "11111111-1111-4111-8111-111111111111");
    assert_eq!(event.event_type, "message.delivered");
    assert_eq!(event.occurred_at, "2026-10-27T10:00:00Z");
    assert_eq!(event.app_id, APP_ID);
    assert_eq!(event.endpoint_id, ENDPOINT_ID);
    assert_eq!(event.data["messageId"], "cmsg000000000000000000");
}

#[tokio::test]
async fn a_delivery_for_another_audience_is_refused() {
    let sealed = Delivery {
        audience: "someone-else",
        ..genuine()
    }
    .seal()
    .await;
    assert_eq!(opener().open(&sealed).await, Err(WebhookError::Unverified));
}

/// The attack the app id exists for. One gateway key signs every app's
/// deliveries, so a delivery sealed for *someone else's* endpoint that names
/// this receiver's audience verifies in every other respect: the signature,
/// the audience, the route, the contract, the clock, a nonce this opener has
/// never seen. Only the app id inside the signed envelope says it was not
/// made for this receiver.
#[tokio::test]
async fn a_delivery_made_for_another_app_is_refused_though_everything_else_verifies() {
    let sealed = Delivery {
        body: OTHER_APP_BODY,
        ..genuine()
    }
    .seal()
    .await;
    assert_eq!(opener().open(&sealed).await, Err(WebhookError::Unverified));
}

#[tokio::test]
async fn a_delivery_made_through_another_endpoint_is_refused_when_the_opener_pins_one() {
    let sealed = genuine().seal().await;
    let pinned = WebhookOpener::builder(&keys(), AUDIENCE, ROUTE, APP_ID)
        .endpoint_id("cendpointother0000000")
        .build()
        .unwrap();
    assert_eq!(pinned.open(&sealed).await, Err(WebhookError::Unverified));
    // Without a pinned endpoint the same body opens: the endpoint is optional,
    // the app is not.
    assert!(opener().open(&genuine().seal().await).await.is_ok());
    let matching = WebhookOpener::builder(&keys(), AUDIENCE, ROUTE, APP_ID)
        .endpoint_id(ENDPOINT_ID)
        .build()
        .unwrap();
    assert!(matching.open(&genuine().seal().await).await.is_ok());
}

/// Rewriting the app id inside a genuine delivery to this receiver's own fails
/// the signature: the id is part of what the gateway signed, not a header.
#[tokio::test]
async fn an_app_id_rewritten_in_flight_fails_the_signature() {
    let sealed = Delivery {
        body: OTHER_APP_BODY,
        ..genuine()
    }
    .seal()
    .await;
    let from = b"cappother000000000000";
    let to = b"capp00000000000000000";
    assert_eq!(from.len(), to.len());
    let at = sealed
        .windows(from.len())
        .position(|window| window == from)
        .expect("the app id is in the signed bytes");
    let mut forged = sealed.clone();
    forged[at..at + to.len()].copy_from_slice(to);
    assert_ne!(forged, sealed);
    assert_eq!(opener().open(&forged).await, Err(WebhookError::Unverified));
}

#[tokio::test]
async fn a_delivery_for_another_route_is_refused() {
    let sealed = Delivery {
        route: "/webhooks/other",
        ..genuine()
    }
    .seal()
    .await;
    assert_eq!(opener().open(&sealed).await, Err(WebhookError::Unverified));
}

#[tokio::test]
async fn a_delivery_signed_by_a_key_that_was_not_pinned_is_refused() {
    let sealed = Delivery {
        seed: OTHER_SEED,
        ..genuine()
    }
    .seal()
    .await;
    assert_eq!(opener().open(&sealed).await, Err(WebhookError::Unverified));
}

#[tokio::test]
async fn a_tampered_delivery_is_refused() {
    let mut sealed = genuine().seal().await;
    let last = sealed.len() - 1;
    sealed[last] ^= 0x01;
    assert_eq!(opener().open(&sealed).await, Err(WebhookError::Unverified));
    assert_eq!(
        opener().open(b"not a cose message").await,
        Err(WebhookError::Unverified)
    );
    assert_eq!(opener().open(b"").await, Err(WebhookError::Unverified));
}

#[tokio::test]
async fn a_stale_delivery_is_refused_and_a_fresh_one_after_it_is_not() {
    let stale = Delivery {
        iat: now() - 3_600,
        ..genuine()
    }
    .seal()
    .await;
    let opener = opener();
    assert_eq!(opener.open(&stale).await, Err(WebhookError::Unverified));
    // A retry is sealed afresh, so it opens whatever happened to the first one.
    assert!(opener.open(&genuine().seal().await).await.is_ok());
}

#[tokio::test]
async fn a_replayed_delivery_is_refused() {
    let sealed = genuine().seal().await;
    let opener = opener();
    assert!(opener.open(&sealed).await.is_ok());
    assert_eq!(opener.open(&sealed).await, Err(WebhookError::Unverified));
}

#[tokio::test]
async fn a_delivery_bound_to_another_contract_is_refused() {
    let sealed = Delivery {
        contract: [9; 32],
        ..genuine()
    }
    .seal()
    .await;
    assert_eq!(opener().open(&sealed).await, Err(WebhookError::Unverified));
}

#[tokio::test]
async fn a_delivery_sealed_as_another_payload_type_is_refused() {
    let sealed = Delivery {
        payload_type: "text/plain",
        ..genuine()
    }
    .seal()
    .await;
    assert_eq!(opener().open(&sealed).await, Err(WebhookError::Unverified));
}

#[tokio::test]
async fn a_verified_body_that_is_not_an_envelope_is_malformed_not_unverified() {
    let sealed = Delivery {
        body: r#"{"hello":"world"}"#,
        ..genuine()
    }
    .seal()
    .await;
    assert!(matches!(
        opener().open(&sealed).await,
        Err(WebhookError::Malformed(_))
    ));
}

#[tokio::test]
async fn the_next_pinned_key_opens_too_so_a_rotation_is_not_a_flag_day() {
    // RFC 8032 test vector 2 is the second key `keys()` pins.
    let seed: [u8; 32] = [
        0x4c, 0xcd, 0x08, 0x9b, 0x28, 0xff, 0x96, 0xda, 0x9d, 0xb6, 0xc3, 0x46, 0xec, 0x11, 0x4e,
        0x0f, 0x5b, 0x8a, 0x31, 0x9f, 0x35, 0xab, 0xa6, 0x24, 0xda, 0x8c, 0xf6, 0xed, 0x4f, 0xb8,
        0xa6, 0xfb,
    ];
    let sealed = Delivery { seed, ..genuine() }.seal().await;
    assert!(opener().open(&sealed).await.is_ok());
}

/// A nonce store that cannot be reached is the receiver's outage, not a bad
/// delivery: it must not read as a 401 (vsms would stop retrying nothing, but a
/// receiver that answers 401 for its own outage hides it).
#[tokio::test]
async fn a_failing_nonce_store_is_a_backend_error_not_a_refusal() {
    struct Down;
    #[async_trait::async_trait]
    impl NonceStore for Down {
        async fn record_if_unseen(
            &self,
            _nonce: &str,
            _expires_at: cratestack::chrono::DateTime<cratestack::chrono::Utc>,
        ) -> Result<bool, CratestackError> {
            Err(CratestackError::Internal("redis is down".to_owned()))
        }
    }
    let opener = WebhookOpener::builder(&keys(), AUDIENCE, ROUTE, APP_ID)
        .with_nonce_store(Arc::new(Down))
        .build()
        .unwrap();
    let result = opener.open(&genuine().seal().await).await;
    assert!(
        matches!(result, Err(WebhookError::Backend(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn the_skew_and_the_clock_can_be_set() {
    let sealed = Delivery {
        iat: now() - 3_600,
        ..genuine()
    }
    .seal()
    .await;
    let opener = WebhookOpener::builder(&keys(), AUDIENCE, ROUTE, APP_ID)
        .skew(Duration::from_secs(7_200))
        .build()
        .unwrap();
    assert!(opener.open(&sealed).await.is_ok());

    let sealed = Delivery {
        iat: 1_000,
        ..genuine()
    }
    .seal()
    .await;
    let opener = WebhookOpener::builder(&keys(), AUDIENCE, ROUTE, APP_ID)
        .clock(|| 1_000)
        .build()
        .unwrap();
    assert!(opener.open(&sealed).await.is_ok());
}

#[test]
fn a_misconfigured_opener_says_so_before_anything_is_opened() {
    assert!(WebhookOpener::new(&keys(), "", ROUTE, APP_ID).is_err());
    assert!(WebhookOpener::new(&keys(), AUDIENCE, "webhooks/vsms", APP_ID).is_err());
    // No default accepts any app: an opener without an app id cannot be built.
    assert!(WebhookOpener::new(&keys(), AUDIENCE, ROUTE, "").is_err());
    assert!(WebhookOpener::builder(&keys(), AUDIENCE, ROUTE, APP_ID)
        .endpoint_id("")
        .build()
        .is_err());
}

/// The bytes the gateway's own `sms-webhook` crate produced for these inputs
/// (`backends/crates/sms-webhook/tests/sealed_vector.rs`), opened by this SDK.
#[tokio::test]
async fn the_gateways_own_vector_opens() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sealed_delivery_vector.json"
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
    assert_eq!(vector["contractSha256"], WEBHOOK_CONTRACT_SHA_HEX);
    let iat = vector["iat"].as_i64().unwrap();
    let opener = WebhookOpener::builder(
        &keys(),
        vector["audience"].as_str().unwrap(),
        vector["route"].as_str().unwrap(),
        vector["appId"].as_str().unwrap(),
    )
    .endpoint_id(vector["endpointId"].as_str().unwrap())
    .clock(move || iat)
    .build()
    .unwrap();
    let event = opener.open(&hex("sealedHex")).await.unwrap();
    let plaintext: serde_json::Value =
        serde_json::from_str(vector["plaintext"].as_str().unwrap()).unwrap();
    assert_eq!(event.id, plaintext["id"].as_str().unwrap());
    assert_eq!(event.event_id, plaintext["eventId"].as_str().unwrap());
    assert_eq!(event.app_id, vector["appId"].as_str().unwrap());
    assert_eq!(event.endpoint_id, vector["endpointId"].as_str().unwrap());
    assert_eq!(event.data, plaintext["data"]);
}
