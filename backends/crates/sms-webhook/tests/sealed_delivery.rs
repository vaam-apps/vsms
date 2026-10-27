//! A sealed delivery against a real cratestack `EnvelopeLayer` (`Required`), the
//! layer vaam runs on `/webhooks/vsms` (VA-5). Nothing here mocks the receiver:
//! `sms_test_support::webhook_receiver` puts the framework's own layer in front
//! of an axum router, configured the way `lib.md` tells a receiver to.
//!
//! Plain, fast, no `#[ignore]`: no Docker, no Postgres.

use std::sync::Arc;

use cratestack::cose::{CoseEnvelope, CoseMode, Ed25519Signer, StaticVerifierResolver};
use sms_test_support::webhook_receiver::WebhookReceiver;
use sms_webhook::{CONTRACT_SHA, SealedDelivery, Sealer};

const ROUTE: &str = "/webhooks/vsms";
const OTHER_ROUTE: &str = "/webhooks/other";
const AUDIENCE: &str = "vaam";
const BODY: &str = r#"{"id":"a","eventId":"11111111-1111-4111-8111-111111111111","type":"message.delivered","occurredAt":"2026-10-27T10:00:00Z","appId":"capp00000000000000000","endpointId":"cendpoint0000000000000","data":{"state":"delivered"}}"#;
/// What the gateway seals for an endpoint of *another* app that names the same
/// audience: everything but the app id is alike.
const OTHER_APP_BODY: &str = r#"{"id":"a","eventId":"11111111-1111-4111-8111-111111111111","type":"message.delivered","occurredAt":"2026-10-27T10:00:00Z","appId":"cappother000000000000","endpointId":"cendpoint0000000000000","data":{"state":"delivered"}}"#;

fn vsms_key() -> Ed25519Signer {
    Ed25519Signer::from_seed(&[11; 32])
}

fn pinned(signer: &Ed25519Signer) -> [u8; 32] {
    signer.verify_key().ed25519_bytes().unwrap()
}

/// A receiver pinned to `signer`'s public key, serving both routes under
/// `contract`.
fn receiver(signer: &Ed25519Signer, audience: &str, contract: [u8; 32]) -> WebhookReceiver {
    WebhookReceiver::new(&[pinned(signer)], audience, &[ROUTE, OTHER_ROUTE], contract)
}

async fn seal(audience: &str, route: &str) -> SealedDelivery {
    Sealer::new(vsms_key())
        .unwrap()
        .seal(
            audience,
            &format!("https://api.vaam.store{route}"),
            BODY.as_bytes(),
        )
        .await
        .unwrap()
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

#[tokio::test]
async fn a_delivery_is_accepted_and_the_handler_sees_the_plain_envelope() {
    let receiver = receiver(&vsms_key(), AUDIENCE, CONTRACT_SHA);
    let delivery = seal(AUDIENCE, ROUTE).await;
    let verdict = receiver
        .deliver(ROUTE, &delivery.headers, &delivery.body)
        .await;
    assert_eq!(verdict.status, 200);
    assert_eq!(receiver.bodies(), vec![BODY.to_owned()]);
    // The receiver seals its answer; vsms does not read it (lib.md, "The answer").
    assert!(verdict.sealed_answer);
}

#[tokio::test]
async fn a_delivery_for_another_audience_is_refused() {
    let receiver = receiver(&vsms_key(), AUDIENCE, CONTRACT_SHA);
    let delivery = seal("someone-else", ROUTE).await;
    let verdict = receiver
        .deliver(ROUTE, &delivery.headers, &delivery.body)
        .await;
    assert_eq!(verdict.status, 401);
    assert_eq!(receiver.bodies(), Vec::<String>::new());
}

/// The limit of what a receiver's `EnvelopeLayer` can know, and why `lib.md`
/// tells it to check `appId` in its handler. The gateway has one key for every
/// app, and a seal binds the audience, not the app: a delivery made for another
/// app's endpoint that names this receiver's audience passes every check the
/// layer has. The app id is in the *signed* body, so the handler can read it.
#[tokio::test]
async fn the_layer_cannot_tell_another_apps_delivery_but_the_signed_body_can() {
    let receiver = receiver(&vsms_key(), AUDIENCE, CONTRACT_SHA);
    let delivery = Sealer::new(vsms_key())
        .unwrap()
        .seal(
            AUDIENCE,
            &format!("https://evil.example{ROUTE}"),
            OTHER_APP_BODY.as_bytes(),
        )
        .await
        .unwrap();
    let verdict = receiver
        .deliver(ROUTE, &delivery.headers, &delivery.body)
        .await;
    assert_eq!(verdict.status, 200, "the layer alone accepts it");
    let seen = receiver.bodies();
    assert_eq!(seen.len(), 1);
    let envelope: serde_json::Value = serde_json::from_str(&seen[0]).unwrap();
    assert_ne!(
        envelope["appId"], "capp00000000000000000",
        "so the handler's own comparison with its app id is what refuses it"
    );
}

#[tokio::test]
async fn an_app_id_rewritten_in_the_sealed_bytes_fails_the_seal() {
    let receiver = receiver(&vsms_key(), AUDIENCE, CONTRACT_SHA);
    let mut delivery = Sealer::new(vsms_key())
        .unwrap()
        .seal(
            AUDIENCE,
            &format!("https://h{ROUTE}"),
            OTHER_APP_BODY.as_bytes(),
        )
        .await
        .unwrap();
    let from = b"cappother000000000000";
    let at = delivery
        .body
        .windows(from.len())
        .position(|window| window == from)
        .expect("the app id is in the signed bytes");
    delivery.body[at..at + from.len()].copy_from_slice(b"capp00000000000000000");
    let verdict = receiver
        .deliver(ROUTE, &delivery.headers, &delivery.body)
        .await;
    assert_eq!(verdict.status, 401);
    assert_eq!(receiver.bodies(), Vec::<String>::new());
}

#[tokio::test]
async fn a_tampered_body_is_refused() {
    let receiver = receiver(&vsms_key(), AUDIENCE, CONTRACT_SHA);
    let mut delivery = seal(AUDIENCE, ROUTE).await;
    let last = delivery.body.len() - 1;
    delivery.body[last] ^= 0x01;
    let verdict = receiver
        .deliver(ROUTE, &delivery.headers, &delivery.body)
        .await;
    assert_eq!(verdict.status, 401);
    assert_eq!(receiver.bodies(), Vec::<String>::new());
}

#[tokio::test]
async fn a_replayed_delivery_is_refused_and_a_retry_is_a_new_request() {
    let receiver = receiver(&vsms_key(), AUDIENCE, CONTRACT_SHA);
    let first = seal(AUDIENCE, ROUTE).await;
    assert_eq!(
        receiver
            .deliver(ROUTE, &first.headers, &first.body)
            .await
            .status,
        200
    );
    // The same bytes again: the nonce store has seen them.
    assert_eq!(
        receiver
            .deliver(ROUTE, &first.headers, &first.body)
            .await
            .status,
        401
    );
    // A retry seals again (fresh nonce, fresh clock) and opens.
    let retry = seal(AUDIENCE, ROUTE).await;
    assert_ne!(retry.body, first.body);
    assert_eq!(
        receiver
            .deliver(ROUTE, &retry.headers, &retry.body)
            .await
            .status,
        200
    );
    assert_eq!(receiver.bodies().len(), 2);
}

#[tokio::test]
async fn a_delivery_sealed_for_one_route_does_not_open_at_another() {
    let receiver = receiver(&vsms_key(), AUDIENCE, CONTRACT_SHA);
    let delivery = seal(AUDIENCE, ROUTE).await;
    let verdict = receiver
        .deliver(OTHER_ROUTE, &delivery.headers, &delivery.body)
        .await;
    assert_eq!(verdict.status, 401);
    assert_eq!(receiver.bodies(), Vec::<String>::new());
}

#[tokio::test]
async fn a_delivery_signed_by_a_key_the_receiver_did_not_pin_is_refused() {
    let receiver = receiver(&Ed25519Signer::from_seed(&[33; 32]), AUDIENCE, CONTRACT_SHA);
    let delivery = seal(AUDIENCE, ROUTE).await;
    let verdict = receiver
        .deliver(ROUTE, &delivery.headers, &delivery.body)
        .await;
    assert_eq!(verdict.status, 401);
    assert_eq!(receiver.bodies(), Vec::<String>::new());
}

#[tokio::test]
async fn a_stale_delivery_is_refused() {
    let receiver = receiver(&vsms_key(), AUDIENCE, CONTRACT_SHA);
    // Sealed an hour ago: far outside the layer's 300 s skew.
    let hour_ago = now() - 3600;
    let cose = CoseEnvelope::client(
        CoseMode::Sign1,
        Arc::new(vsms_key()),
        Arc::new(StaticVerifierResolver::new()),
    )
    .clock(move || hour_ago)
    .build()
    .unwrap();
    let delivery = Sealer::from_envelope(cose)
        .unwrap()
        .seal(AUDIENCE, &format!("https://h{ROUTE}"), BODY.as_bytes())
        .await
        .unwrap();
    let verdict = receiver
        .deliver(ROUTE, &delivery.headers, &delivery.body)
        .await;
    assert_eq!(verdict.status, 401);
    assert_eq!(receiver.bodies(), Vec::<String>::new());
}

#[tokio::test]
async fn a_receiver_that_does_not_declare_the_contract_does_not_serve_the_delivery() {
    // The layer's unsigned `426 contract_unsupported`: the digest is bound.
    let receiver = receiver(&vsms_key(), AUDIENCE, [9; 32]);
    let delivery = seal(AUDIENCE, ROUTE).await;
    let verdict = receiver
        .deliver(ROUTE, &delivery.headers, &delivery.body)
        .await;
    assert_eq!(verdict.status, 426);
    assert_eq!(receiver.bodies(), Vec::<String>::new());
}

#[tokio::test]
async fn an_unsealed_request_never_reaches_the_handler() {
    let receiver = receiver(&vsms_key(), AUDIENCE, CONTRACT_SHA);
    let plain = [("content-type".to_owned(), "application/json".to_owned())];
    let verdict = receiver.deliver(ROUTE, &plain, BODY.as_bytes()).await;
    assert_ne!(verdict.status, 200);
    assert_eq!(receiver.bodies(), Vec::<String>::new());
}

/// No HMAC header is sent: a delivery's headers are the seal's alone.
#[tokio::test]
async fn a_delivery_carries_no_signature_or_timestamp_header() {
    let delivery = seal(AUDIENCE, ROUTE).await;
    for (name, _) in &delivery.headers {
        let name = name.to_ascii_lowercase();
        assert!(
            !name.contains("signature")
                && !name.contains("timestamp")
                && !name.starts_with("x-sms"),
            "{name}"
        );
    }
}
