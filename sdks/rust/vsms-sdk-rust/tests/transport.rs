//! What the SDK puts on the wire and what it does with a refusal, against a
//! local stand-in for the gateway (`wiremock`): no stack, no database, runs
//! under a plain `cargo test`.
//!
//! The server half of the same claims - that a gateway opens exactly what this
//! seals and answers with a message this verifies - is proven against the real
//! gateway by `tests/live_gateway.rs` and by `backends/crates/sms-api/tests/
//! envelope_live_postgres.rs`, whose client uses the identical
//! `ClientEnvelope::seal_call`.

use vsms_sdk::schema::SendMessageInput;
use vsms_sdk::{ServerKeys, SigningKey, VsmsClient};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn keys() -> ServerKeys {
    ServerKeys::from_base64url(
        "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
        "PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw",
    )
    .expect("two valid public keys")
}

fn args() -> SendMessageInput {
    SendMessageInput {
        to: "+237677123456".to_owned(),
        body: "a sealed send".to_owned(),
        senderId: Some("VSMS".to_owned()),
        class: None,
        clientRef: None,
        scheduledAt: None,
        validityMinutes: None,
    }
}

/// The gateway's own refusal of a request it did not accept is a plain 401
/// with no COSE body (the envelope's coarse, unsigned answer, never a signed
/// one: a replay must not earn a signed "no" for a request that already ran).
fn unsigned_401() -> ResponseTemplate {
    ResponseTemplate::new(401).set_body_json(serde_json::json!({
        "code": "UNAUTHORIZED",
        "message": "unauthorized",
    }))
}

#[tokio::test]
async fn send_message_is_sealed_and_carries_no_bearer() {
    let gateway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/$procs/sendMessage"))
        .respond_with(unsigned_401())
        .mount(&gateway)
        .await;
    let client =
        VsmsClient::signed(gateway.uri(), SigningKey::generate(), keys()).expect("a client");

    let _ = client.send_message(args(), Some("retry-safe-key")).await;

    let requests = gateway.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let header = |name: &str| {
        request
            .headers
            .get(name)
            .map(|value| value.to_str().unwrap().to_owned())
    };
    assert_eq!(
        header("content-type").as_deref(),
        Some("application/cose; cose-type=\"cose-sign1\""),
        "the body is a COSE_Sign1 message"
    );
    assert_eq!(
        header("cratestack-payload-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(
        header("cratestack-payload-accept").as_deref(),
        Some("application/json")
    );
    assert_eq!(header("idempotency-key").as_deref(), Some("retry-safe-key"));
    assert!(
        header("authorization").is_none(),
        "there is no bearer to send"
    );
    assert!(
        serde_json::from_slice::<serde_json::Value>(&request.body).is_err(),
        "the body is a COSE message, not a plain JSON document"
    );
}

/// There is no refresh any more: an unsigned refusal is surfaced after exactly
/// one attempt, never retried (a retry is a new request with a new nonce, the
/// caller's decision). This replaces the token-era "a deliberate 401 triggers
/// exactly one refresh" test.
#[tokio::test]
async fn an_unsigned_401_is_surfaced_after_exactly_one_attempt() {
    let gateway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/$procs/sendMessage"))
        .respond_with(unsigned_401())
        .mount(&gateway)
        .await;
    let client =
        VsmsClient::signed(gateway.uri(), SigningKey::generate(), keys()).expect("a client");

    let error = client
        .send_message(args(), None)
        .await
        .expect_err("an unsigned answer is never a success");

    assert!(error.is_unauthorized(), "{error}");
    assert_eq!(error.status(), Some(401));
    assert_eq!(
        gateway.received_requests().await.expect("recorded").len(),
        1,
        "the SDK does not retry a refused signature"
    );
}

/// A client that has a key never falls back to plain traffic: an answer that is
/// not sealed is an error whatever its status, so a proxy that strips the
/// envelope cannot turn this into an unauthenticated client.
#[tokio::test]
async fn a_plain_200_is_not_believed() {
    let gateway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/$procs/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "messageId": "forged",
            "state": "queued",
        })))
        .mount(&gateway)
        .await;
    let client =
        VsmsClient::signed(gateway.uri(), SigningKey::generate(), keys()).expect("a client");

    let error = client
        .send_message(args(), None)
        .await
        .expect_err("an unsigned 200 must not be accepted");

    assert!(
        error.to_string().contains("without a COSE envelope"),
        "{error}"
    );
}

#[test]
fn building_a_client_needs_all_three_inputs() {
    let missing = |result: Result<VsmsClient, vsms_sdk::SdkError>, what: &str| {
        let error = result.err().expect("an incomplete builder is refused");
        assert!(error.to_string().contains(what), "{error}");
    };
    missing(
        VsmsClient::builder()
            .signing_key(SigningKey::generate())
            .server_keys(keys())
            .build(),
        "base_url",
    );
    missing(
        VsmsClient::builder()
            .base_url("http://127.0.0.1:1")
            .server_keys(keys())
            .build(),
        "signing key",
    );
    missing(
        VsmsClient::builder()
            .base_url("http://127.0.0.1:1")
            .signing_key(SigningKey::generate())
            .build(),
        "pinned keys",
    );
}
