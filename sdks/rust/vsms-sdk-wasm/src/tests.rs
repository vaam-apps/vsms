//! Native tests of the sealing path, against the responder: cratestack's
//! server envelope, which shares no code with [`crate::core`].

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use cratestack::cose::{CoseSigner, Ed25519Signer, ExternalSigner, P256Signer};

use crate::core::{
    concrete_path, Algorithm, OpenError, SealError, Sealed, Sealer, SealerKey, ServerKeys,
};
use crate::responder::{OpenedRequest, Responder};

const CLIENT_SEED: [u8; 32] = [7; 32];
const SERVER_SEED: [u8; 32] = [9; 32];
const NEXT_SEED: [u8; 32] = [10; 32];
const STRANGER_SEED: [u8; 32] = [11; 32];
const SCALAR: [u8; 32] = [5; 32];

fn public(seed: &[u8; 32]) -> [u8; 32] {
    Responder::public_key(seed).unwrap()
}

fn server_keys() -> ServerKeys {
    ServerKeys::new(public(&SERVER_SEED), public(&NEXT_SEED)).unwrap()
}

fn client_public() -> Vec<u8> {
    public(&CLIENT_SEED).to_vec()
}

fn esp256_signer() -> (Arc<dyn CoseSigner>, Vec<u8>) {
    let key = P256Signer::from_scalar(&SCALAR).unwrap();
    let public = key.verify_key().p256_sec1_uncompressed().unwrap().to_vec();
    let signer = ExternalSigner::esp256(&public, move |tbs| {
        let key = key.clone();
        async move { key.sign(&tbs).await }
    })
    .unwrap();
    (Arc::new(signer), public)
}

fn seed_sealer() -> Sealer {
    Sealer::new(SealerKey::Seed(CLIENT_SEED), &server_keys()).unwrap()
}

fn responder(audience: &str) -> Responder {
    Responder::new(&SERVER_SEED, &[client_public()], audience).unwrap()
}

fn header<'a>(sealed: &'a Sealed, name: &str) -> Option<&'a str> {
    sealed
        .headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

async fn open(responder: &Responder, method: &str, sealed: &Sealed) -> Result<OpenedRequest, ()> {
    let (path, query) = match sealed.target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (sealed.target.as_str(), None),
    };
    responder
        .open_request(method, path, query, &sealed.headers, sealed.body.clone())
        .await
}

const SEND: (&str, &str) = ("POST", "/$procs/sendMessage");
const GET: (&str, &str) = ("GET", "/messages/{id}");

async fn send(sealer: &Sealer, key: Option<&str>) -> Sealed {
    sealer
        .seal_call(
            SEND.0,
            SEND.1,
            &[],
            Some(r#"{"args":{"to":"+237677123456","body":"hi"}}"#),
            None,
            key,
        )
        .await
        .unwrap()
}

// ---- what is sealed ---------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_write_is_sealed_with_the_idempotency_key_it_was_given() {
    let sealed = send(&seed_sealer(), Some("idem-1")).await;
    assert!(!sealed.body.is_empty());
    assert!(
        header(&sealed, "content-type")
            .unwrap()
            .starts_with("application/cose"),
        "{:?}",
        sealed.headers
    );
    assert_eq!(header(&sealed, "idempotency-key"), Some("idem-1"));
    assert!(header(&sealed, "cratestack-contract").is_some());
    assert_eq!(sealed.target, "/$procs/sendMessage");
}

#[tokio::test(flavor = "current_thread")]
async fn the_body_is_a_cose_message_that_carries_the_json_signed() {
    let sealed = send(&seed_sealer(), None).await;
    // Sign1 signs, it does not encrypt: the JSON rides inside the message.
    assert!(
        sealed
            .body
            .windows(13)
            .any(|window| window == b"+237677123456"),
        "the signed payload is the JSON"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_seals_of_one_call_differ_so_neither_is_a_replay_of_the_other() {
    let sealer = seed_sealer();
    let first = send(&sealer, None).await;
    let second = send(&sealer, None).await;
    assert_ne!(first.body, second.body);
}

#[tokio::test(flavor = "current_thread")]
async fn a_read_carries_its_parameter_in_the_path_and_its_query_after_it() {
    let sealed = seed_sealer()
        .seal_call(
            GET.0,
            GET.1,
            &["m 1/2"],
            None,
            Some("fields=id,state"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(sealed.target, "/messages/m%201%2F2?fields=id,state");
}

// ---- what is refused before anything is signed ------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_route_the_schema_does_not_serve_is_unbound() {
    for (method, template) in [
        ("GET", "/nope"),
        ("DELETE", "/$procs/sendMessage"),
        // A concrete path is not a template.
        ("GET", "/messages/m1"),
        // Neither is a table key: the method is not part of the route.
        ("GET", "GET /messages/{id}"),
        ("GET", "messages/{id}"),
    ] {
        let error = seed_sealer()
            .seal_call(method, template, &["m1"], None, None, None)
            .await
            .unwrap_err();
        assert!(
            matches!(error, SealError::Unbound(_)),
            "{method} {template}: {error:?}"
        );
    }
}

#[test]
fn parameters_must_fit_the_route() {
    assert_eq!(
        concrete_path("/messages/{id}", &["m1"]).unwrap(),
        "/messages/m1"
    );
    assert_eq!(
        concrete_path("/$procs/sendMessage", &[]).unwrap(),
        "/$procs/sendMessage"
    );
    for (template, params) in [
        ("/messages/{id}", &[][..]),
        ("/messages/{id}", &["a", "b"][..]),
        ("/$procs/sendMessage", &["a"][..]),
        ("/messages/{id}", &[""][..]),
    ] {
        let error = concrete_path(template, params).unwrap_err();
        assert!(
            matches!(error, SealError::Unbound(_)),
            "{template} {params:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_idempotency_key_that_would_not_survive_a_hop_is_refused() {
    let error = seed_sealer()
        .seal_call(SEND.0, SEND.1, &[], Some("{}"), None, Some("two\nlines"))
        .await
        .unwrap_err();
    assert!(matches!(error, SealError::Seal(_)), "{error:?}");
}

#[test]
fn a_seed_is_never_printed() {
    let text = format!("{:?}", SealerKey::Seed([0xAB; 32]));
    assert!(!text.contains("171") && !text.contains("ab"), "{text}");
    assert!(text.contains("redacted"));
}

#[test]
fn server_keys_must_be_two_distinct_points() {
    let a = public(&SERVER_SEED);
    assert!(ServerKeys::new(a, a).is_err(), "the same key twice");
    // y = 2 has no x on the curve, so these 32 bytes decompress to nothing.
    let mut not_a_point = [0u8; 32];
    not_a_point[0] = 2;
    assert!(ServerKeys::new(not_a_point, a).is_err());
    assert!(ServerKeys::new(a, not_a_point).is_err());
}

// ---- the server's view of what was sealed -----------------------------

#[tokio::test(flavor = "current_thread")]
async fn the_server_rebuilds_the_binding_an_ed25519_seed_signed() {
    let sealer = seed_sealer();
    let sealed = send(&sealer, Some("idem-2")).await;
    let opened = open(&responder("vsms"), SEND.0, &sealed).await.unwrap();
    assert_eq!(opened.route(), "/$procs/sendMessage");
    assert_eq!(opened.idempotency_key.as_deref(), Some("idem-2"));
    assert!(opened.payload.contains("+237677123456"));
    assert_eq!(sealer.algorithm(), Algorithm::Ed25519);
    assert_eq!(sealer.public_key().len(), 32);
}

#[tokio::test(flavor = "current_thread")]
async fn a_read_binds_the_parameter_the_url_carries() {
    let sealed = seed_sealer()
        .seal_call(GET.0, GET.1, &["m 1/2"], None, Some("b=2&a=1"), None)
        .await
        .unwrap();
    let opened = open(&responder("vsms"), GET.0, &sealed).await.unwrap();
    assert_eq!(opened.route(), "/messages/{id}");
    assert_eq!(opened.path_params(), ["m 1/2"]);
}

#[tokio::test(flavor = "current_thread")]
async fn an_ed25519_key_behind_a_callback_signs_the_same_binding() {
    let key = Ed25519Signer::from_seed(&CLIENT_SEED);
    let signer = ExternalSigner::ed25519(&public(&CLIENT_SEED), move |tbs| {
        let key = key.clone();
        async move { key.sign(&tbs).await }
    })
    .unwrap();
    let sealer = Sealer::new(
        SealerKey::Ed25519 {
            public_key: public(&CLIENT_SEED),
            signer: Arc::new(signer),
        },
        &server_keys(),
    )
    .unwrap();
    let sealed = send(&sealer, None).await;
    assert!(open(&responder("vsms"), SEND.0, &sealed).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn a_p256_key_behind_a_callback_signs_esp256() {
    let (signer, public) = esp256_signer();
    let sealer = Sealer::new(
        SealerKey::Esp256 {
            public_key: public.clone(),
            signer,
        },
        &server_keys(),
    )
    .unwrap();
    assert_eq!(sealer.algorithm(), Algorithm::Esp256);
    // The canonical form a `ConsoleKey` row holds: SEC1 uncompressed.
    assert_eq!(sealer.public_key(), public.as_slice());
    assert_eq!(sealer.public_key().len(), 65);
    let sealed = send(&sealer, None).await;
    let server = Responder::new(&SERVER_SEED, &[public], "vsms").unwrap();
    assert!(open(&server, SEND.0, &sealed).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn a_compressed_p256_key_is_reported_in_its_canonical_uncompressed_form() {
    let key = P256Signer::from_scalar(&SCALAR).unwrap();
    let uncompressed = key.verify_key().p256_sec1_uncompressed().unwrap();
    let compressed = {
        use cratestack::cose::CoseVerifyKey;
        // 0x02/0x03 || X, by the parity of Y.
        let mut out = vec![if uncompressed[64] & 1 == 0 { 2 } else { 3 }];
        out.extend_from_slice(&uncompressed[1..33]);
        assert!(CoseVerifyKey::p256_sec1(&out).is_ok());
        out
    };
    let signer = ExternalSigner::esp256(&compressed, move |tbs| {
        let key = key.clone();
        async move { key.sign(&tbs).await }
    })
    .unwrap();
    let sealer = Sealer::new(
        SealerKey::Esp256 {
            public_key: compressed,
            signer: Arc::new(signer),
        },
        &server_keys(),
    )
    .unwrap();
    assert_eq!(sealer.public_key(), uncompressed.as_slice());
}

#[tokio::test(flavor = "current_thread")]
async fn an_esp256_signer_whose_public_key_is_not_a_point_is_refused() {
    let (signer, _) = esp256_signer();
    let error = Sealer::new(
        SealerKey::Esp256 {
            public_key: vec![4; 65],
            signer,
        },
        &server_keys(),
    )
    .unwrap_err();
    assert!(matches!(error, SealError::Seal(_)), "{error:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_key_registered_under_the_other_algorithm_is_not_a_signer() {
    // The gateway reads `ClientKey` for Ed25519 and `ConsoleKey` for ESP256,
    // and a key of one kind never resolves for the other. Here the server has
    // only the Ed25519 key registered and the request is signed under ESP256.
    let (signer, public) = esp256_signer();
    let sealer = Sealer::new(
        SealerKey::Esp256 {
            public_key: public,
            signer,
        },
        &server_keys(),
    )
    .unwrap();
    let sealed = send(&sealer, None).await;
    assert!(open(&responder("vsms"), SEND.0, &sealed).await.is_err());
}

// ---- what the server refuses ------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_request_addressed_to_another_audience_is_refused() {
    let sealed = send(&seed_sealer(), None).await;
    assert!(open(&responder("someone-else"), SEND.0, &sealed)
        .await
        .is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_for_one_message_cannot_be_replayed_at_another() {
    let sealed = seed_sealer()
        .seal_call(GET.0, GET.1, &["m1"], None, None, None)
        .await
        .unwrap();
    let server = responder("vsms");
    let moved = server
        .open_request(
            GET.0,
            "/messages/m2",
            None,
            &sealed.headers,
            sealed.body.clone(),
        )
        .await;
    assert!(moved.is_err());
    // The same bytes at the path they were sealed for are accepted.
    assert!(open(&server, GET.0, &sealed).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_is_accepted_once() {
    let sealed = send(&seed_sealer(), None).await;
    let server = responder("vsms");
    assert!(open(&server, SEND.0, &sealed).await.is_ok());
    assert!(open(&server, SEND.0, &sealed).await.is_err(), "a replay");
}

#[tokio::test(flavor = "current_thread")]
async fn the_idempotency_key_is_bound_as_sent() {
    let sealed = send(&seed_sealer(), Some("idem-3")).await;
    let swapped: Vec<(String, String)> = sealed
        .headers
        .iter()
        .map(|(name, value)| {
            if name == "idempotency-key" {
                (name.clone(), "idem-4".to_owned())
            } else {
                (name.clone(), value.clone())
            }
        })
        .collect();
    let refused = responder("vsms")
        .open_request(
            SEND.0,
            "/$procs/sendMessage",
            None,
            &swapped,
            sealed.body.clone(),
        )
        .await;
    assert!(refused.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn the_query_is_bound() {
    let seal = || async {
        seed_sealer()
            .seal_call(GET.0, GET.1, &["m1"], None, Some("fields=id"), None)
            .await
            .unwrap()
    };
    let server = responder("vsms");
    // Dropped, or changed, after sealing: refused. (A client that never sealed
    // the query would be accepted by the first, which is the point of asking.)
    let sealed = seal().await;
    let dropped = server
        .open_request(
            GET.0,
            "/messages/m1",
            None,
            &sealed.headers,
            sealed.body.clone(),
        )
        .await;
    assert!(dropped.is_err(), "a query dropped in transit");
    let sealed = seal().await;
    let changed = server
        .open_request(
            GET.0,
            "/messages/m1",
            Some("fields=body"),
            &sealed.headers,
            sealed.body.clone(),
        )
        .await;
    assert!(changed.is_err(), "a query changed in transit");
    // Sent as sealed: accepted.
    let sealed = seal().await;
    assert!(open(&server, GET.0, &sealed).await.is_ok());
}

// ---- what the client refuses ------------------------------------------

async fn answered(
    server: &Responder,
    sealer: &Sealer,
    status: u16,
    body: &str,
) -> (Sealed, Vec<(String, String)>, Vec<u8>) {
    let sealed = send(sealer, None).await;
    let opened = open(server, SEND.0, &sealed).await.unwrap();
    let (headers, bytes) = opened.seal_response(status, body).await.unwrap();
    (sealed, headers, bytes)
}

#[tokio::test(flavor = "current_thread")]
async fn a_sealed_answer_opens_to_its_payload() {
    let (mut sealed, headers, bytes) = answered(
        &responder("vsms"),
        &seed_sealer(),
        200,
        r#"{"messageId":"m1"}"#,
    )
    .await;
    let payload = sealed.open_response(200, &headers, bytes).await.unwrap();
    assert_eq!(payload, br#"{"messageId":"m1"}"#);
}

#[tokio::test(flavor = "current_thread")]
async fn an_error_answer_is_sealed_too_and_opens_with_its_status() {
    let (mut sealed, headers, bytes) = answered(
        &responder("vsms"),
        &seed_sealer(),
        403,
        r#"{"code":"FORBIDDEN"}"#,
    )
    .await;
    assert!(sealed.open_response(403, &headers, bytes).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_sealed_under_the_next_key_is_trusted() {
    let sealer = seed_sealer();
    let server = Responder::new(&NEXT_SEED, &[client_public()], "vsms").unwrap();
    let (mut sealed, headers, bytes) = answered(&server, &sealer, 200, "{}").await;
    assert!(sealed.open_response(200, &headers, bytes).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_sealed_under_a_key_that_is_not_pinned_is_unverified() {
    let server = Responder::new(&STRANGER_SEED, &[client_public()], "vsms").unwrap();
    let (mut sealed, headers, bytes) = answered(&server, &seed_sealer(), 200, "{}").await;
    let error = sealed
        .open_response(200, &headers, bytes)
        .await
        .unwrap_err();
    assert_eq!(error, OpenError::Unverified);
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_to_another_request_is_unverified() {
    let sealer = seed_sealer();
    let server = responder("vsms");
    let (_first, headers, bytes) = answered(&server, &sealer, 200, r#"{"messageId":"m1"}"#).await;
    let mut second = send(&sealer, None).await;
    let error = second
        .open_response(200, &headers, bytes)
        .await
        .unwrap_err();
    assert_eq!(error, OpenError::Unverified);
}

#[tokio::test(flavor = "current_thread")]
async fn the_status_an_answer_is_sealed_under_is_the_status_it_must_arrive_with() {
    let (mut sealed, headers, bytes) =
        answered(&responder("vsms"), &seed_sealer(), 403, "{}").await;
    let error = sealed
        .open_response(200, &headers, bytes)
        .await
        .unwrap_err();
    assert_eq!(error, OpenError::Unverified);
}

#[tokio::test(flavor = "current_thread")]
async fn a_plain_answer_is_unsigned_whatever_its_status() {
    for status in [200, 201, 401, 500] {
        let mut sealed = send(&seed_sealer(), None).await;
        let error = sealed
            .open_response(
                status,
                &[("content-type".into(), "application/json".into())],
                br#"{"messageId":"m1","state":"queued"}"#.to_vec(),
            )
            .await
            .unwrap_err();
        assert_eq!(error, OpenError::Unsigned { status }, "{status}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_is_opened_once() {
    let mut sealed = send(&seed_sealer(), None).await;
    let _ = sealed.open_response(401, &[], Vec::new()).await;
    let error = sealed
        .open_response(401, &[], Vec::new())
        .await
        .unwrap_err();
    assert!(matches!(error, OpenError::Other(_)), "{error:?}");
}
