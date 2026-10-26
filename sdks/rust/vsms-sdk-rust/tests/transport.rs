//! What the SDK puts on the wire and what it does with a refusal, against a
//! local stand-in for the gateway (`wiremock`): no stack, no database, runs
//! under a plain `cargo test`.
//!
//! The server half of the same claims - that a gateway opens exactly what this
//! seals and answers with a message this verifies - is proven against the real
//! gateway by `tests/live_gateway.rs` and by `backends/crates/sms-api/tests/
//! envelope_live_postgres.rs`, whose client uses the identical
//! `ClientEnvelope::seal_call`.

use std::borrow::Cow;
use std::sync::Arc;

use cratestack::client_rust::{ClientError, EnvelopeError};
use cratestack::cose::{
    request_digest, CoseEnvelope, CoseMode, CoseVerifyKey, Ed25519Signer, StaticVerifierResolver,
};
use cratestack::{Binding, BoundHeaders, InMemoryNonceStore, PathParams, ResponseBinding};
use vsms_sdk::schema::SendMessageInput;
use vsms_sdk::{SdkError, ServerKeys, SigningKey, VsmsClient};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// RFC 8032 test vector 1's secret key; its public key is the first one `keys()`
/// pins, so a gateway sealing with it is the genuine gateway for these tests.
const PINNED_SERVER_SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];

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

const COSE_SIGN1: &str = "application/cose; cose-type=\"cose-sign1\"";

/// A stand-in gateway with a real server envelope: it opens the sealed request
/// from `client`, and answers `200` with a correctly bound `SendMessageResult`
/// sealed by the key of `server_seed`. Whether the SDK believes the answer then
/// depends on one thing only, whether that key is one it pinned.
struct SealingGateway {
    server_seed: [u8; 32],
    client: CoseVerifyKey,
}

impl SealingGateway {
    fn new(server_seed: [u8; 32], client: &SigningKey) -> Self {
        let public = base64_url_decode(&client.public_key_b64u());
        Self {
            server_seed,
            client: CoseVerifyKey::ed25519(&public).expect("a valid client public key"),
        }
    }

    async fn answer(&self, body: Vec<u8>) -> Vec<u8> {
        let contract = *cratestack::find_contract(
            vsms_sdk::schema::OP_CONTRACTS,
            "POST",
            "/$procs/sendMessage",
        )
        .expect("the vendored schema has sendMessage");
        let server = CoseEnvelope::server(
            CoseMode::Sign1,
            Arc::new(Ed25519Signer::from_seed(&self.server_seed)),
            Arc::new(StaticVerifierResolver::new().with_key(self.client.clone())),
            Arc::new(InMemoryNonceStore::new()),
        )
        .build()
        .expect("a server envelope");
        let bind = Binding {
            audience: Cow::Borrowed("vsms"),
            method: Cow::Borrowed("POST"),
            route: Cow::Borrowed("/$procs/sendMessage"),
            path_params: PathParams::EMPTY,
            query: None,
            contract_sha: contract,
            payload_media_type: Cow::Borrowed("application/json"),
            bound_headers: BoundHeaders {
                idempotency_key: None,
                if_match: None,
            },
            response: None,
        };
        server
            .open_request(body.clone().into(), &bind)
            .await
            .expect("the SDK's request opens under the binding sendMessage has");
        let answer = Binding {
            response: Some(ResponseBinding {
                request: request_digest(&body),
                status: 200,
            }),
            ..bind
        };
        server
            .seal_response(
                br#"{"messageId":"m_1","state":"queued","encoding":"gsm7","segments":1,"operator":"mtn","estimatedCostXaf":"0"}"#,
                &answer,
            )
            .await
            .expect("sealing the answer")
            .to_vec()
    }
}

impl Respond for SealingGateway {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        // `respond` is sync and runs on the mock server's own runtime; the
        // sealing is async, so it gets a runtime of its own on a thread of its
        // own rather than blocking inside one.
        let body = request.body.clone();
        let sealed = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    tokio::runtime::Builder::new_current_thread()
                        .build()
                        .expect("a runtime")
                        .block_on(self.answer(body))
                })
                .join()
                .expect("the sealing thread")
        });
        ResponseTemplate::new(200)
            .insert_header("content-type", COSE_SIGN1)
            .insert_header("cratestack-payload-type", "application/json")
            .set_body_bytes(sealed)
    }
}

fn base64_url_decode(text: &str) -> [u8; 32] {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .expect("base64url")
        .try_into()
        .expect("32 bytes")
}

async fn gateway_sealing_with(server_seed: [u8; 32], client: &SigningKey) -> MockServer {
    let gateway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/$procs/sendMessage"))
        .respond_with(SealingGateway::new(server_seed, client))
        .mount(&gateway)
        .await;
    gateway
}

/// The control for the refusal below: an answer sealed by a pinned key is
/// accepted, so the stand-in gateway really does produce a valid answer and the
/// refusal is the SDK's judgement of the key, not of a malformed stub.
#[tokio::test]
async fn an_answer_sealed_by_a_pinned_key_is_accepted() {
    let client_key = SigningKey::generate();
    let gateway = gateway_sealing_with(PINNED_SERVER_SEED, &client_key).await;
    let client = VsmsClient::signed(gateway.uri(), client_key, keys()).expect("a client");

    let outcome = client
        .send_message(args(), None)
        .await
        .expect("an answer signed by a pinned key");

    assert_eq!(outcome.result.messageId, "m_1");
}

/// A genuine COSE answer, correctly bound to the request, but signed by a key
/// the SDK did not pin: whoever holds it is not the gateway.
#[tokio::test]
async fn an_answer_sealed_by_a_key_that_is_not_pinned_is_refused() {
    let client_key = SigningKey::generate();
    let gateway = gateway_sealing_with([0x77; 32], &client_key).await;
    let client = VsmsClient::signed(gateway.uri(), client_key, keys()).expect("a client");

    let error = client
        .send_message(args(), None)
        .await
        .expect_err("an answer from an unpinned key must not be believed");

    assert!(
        matches!(
            error,
            SdkError::Client(ClientError::Envelope(EnvelopeError::Unverified))
        ),
        "{error:?}"
    );
    assert_eq!(
        gateway.received_requests().await.expect("recorded").len(),
        1
    );
}

/// With the default client a 307 is the answer, unsigned, and is refused
/// without a second request.
#[tokio::test]
async fn a_307_is_not_followed_by_the_default_client() {
    let gateway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/$procs/sendMessage"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/elsewhere"))
        .mount(&gateway)
        .await;
    Mock::given(path("/elsewhere"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&gateway)
        .await;
    let client =
        VsmsClient::signed(gateway.uri(), SigningKey::generate(), keys()).expect("a client");

    let error = client
        .send_message(args(), None)
        .await
        .expect_err("a redirect is not an answer");

    assert_eq!(error.status(), Some(307), "{error}");
    let requests = gateway.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 1, "the redirect was not followed");
    assert_eq!(requests[0].url.path(), "/$procs/sendMessage");
}

/// A caller-supplied client that follows redirects has already forwarded the
/// sealed request when the answer arrives (documented on `http_client`); what
/// the SDK guarantees is that it does not believe an answer from the hop, even
/// one the hop makes look entirely successful.
#[tokio::test]
async fn an_answer_from_a_redirect_hop_is_refused() {
    let client_key = SigningKey::generate();
    let gateway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/$procs/sendMessage"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/elsewhere"))
        .mount(&gateway)
        .await;
    // The hop answers with a perfectly valid, pinned-key-sealed answer.
    Mock::given(path("/elsewhere"))
        .respond_with(SealingGateway::new(PINNED_SERVER_SEED, &client_key))
        .mount(&gateway)
        .await;
    let following = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("a redirect-following client");
    let client = VsmsClient::builder()
        .base_url(gateway.uri())
        .signing_key(client_key)
        .server_keys(keys())
        .http_client(following)
        .build()
        .expect("a client");

    let error = client
        .send_message(args(), None)
        .await
        .expect_err("an answer from another URL must not be believed");

    assert!(
        matches!(
            error,
            SdkError::Client(ClientError::Envelope(EnvelopeError::Unverified))
        ),
        "{error:?}"
    );
}
