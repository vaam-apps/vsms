//! The real `sms-provider-aws-sms` adapter against the fake, over real HTTP,
//! with no database: a submit goes out signed, the fake's receipts come back
//! signed, and the adapter's real receiver authenticates and parses them.
//! What this proves is consistency with our reading of AWS, not AWS; see
//! the crate doc.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use rust_decimal::Decimal;
use sms_encoding::SmsEncoding;
use sms_fake_aws::{
    EventStep, FakeAws, FakeAwsConfig, FaultPolicy, Redelivery, RedirectSnsHttp, SubmitDecision,
};
use sms_provider::{
    CallbackDisposition, DeliveryOutcome, ProviderError, RawCallback, SmsProvider, SubmitRequest,
};
use sms_provider_aws_sms::{AwsSmsConfig, AwsSmsProvider, MessageType, SnsHttp};
use url::Url;

fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// What the stand-in endpoint saw for one POST.
#[derive(Debug, Clone)]
enum Seen {
    Ingested(Vec<(String, DeliveryOutcome)>),
    Acknowledged,
    Refused,
    Unavailable,
}

#[derive(Clone)]
struct Endpoint {
    receiver: Arc<OnceReceiver>,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// Statuses to answer before behaving normally, to exercise redelivery.
    fail_first: Arc<Mutex<Vec<StatusCode>>>,
}

/// Set once the fake's address is known, which is after the endpoint's
/// address has to be.
#[derive(Default)]
struct OnceReceiver(std::sync::OnceLock<AwsSmsProvider>);

/// Mirrors `sms-gateway`'s `dlr_handler` minus the database: authenticate,
/// then parse. 200 / 403 / 503 as the gateway answers.
async fn endpoint(State(state): State<Endpoint>, headers: HeaderMap, body: Bytes) -> StatusCode {
    if let Some(status) = state.fail_first.lock().unwrap().pop() {
        return status;
    }
    let raw = RawCallback {
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_owned()))
            .collect(),
        body: body.to_vec(),
    };
    let receiver = state
        .receiver
        .0
        .get()
        .expect("receiver set before any POST");
    let (seen, status) = match receiver.authenticate_callback(&raw).await {
        Ok(CallbackDisposition::Ingest) => {
            let updates = receiver
                .parse_dlr(&raw)
                .expect("an authentic receipt parses");
            (
                Seen::Ingested(
                    updates
                        .into_iter()
                        .map(|u| (u.provider_ref, u.outcome))
                        .collect(),
                ),
                StatusCode::OK,
            )
        }
        Ok(CallbackDisposition::Acknowledged { .. }) => (Seen::Acknowledged, StatusCode::OK),
        Ok(CallbackDisposition::Refused { reason }) => {
            eprintln!("endpoint refused a callback: {reason}");
            (Seen::Refused, StatusCode::FORBIDDEN)
        }
        Err(error) => {
            eprintln!("endpoint could not authenticate a callback: {error}");
            (Seen::Unavailable, StatusCode::SERVICE_UNAVAILABLE)
        }
    };
    state.seen.lock().unwrap().push(seen);
    status
}

struct Harness {
    fake: FakeAws,
    sender: AwsSmsProvider,
    seen: Arc<Mutex<Vec<Seen>>>,
    fail_first: Arc<Mutex<Vec<StatusCode>>>,
}

fn sender_config(fake: &FakeAws, secret: &str, configuration_set: &str) -> AwsSmsConfig {
    AwsSmsConfig {
        region: sms_fake_aws::DEFAULT_REGION.to_owned(),
        access_key_id: sms_fake_aws::DEFAULT_ACCESS_KEY_ID.to_owned(),
        secret_access_key: secret.to_owned(),
        configuration_set: configuration_set.to_owned(),
        origination_identity: None,
        message_type: MessageType::Transactional,
        tps_ceiling: 10.0,
        cost_per_segment_xaf: Decimal::new(30, 0),
        supports_alphanumeric_sender: true,
        endpoint_url: Some(fake.base_url().to_owned()),
        time_to_live: Some(Duration::from_mins(15)),
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_millis(500),
    }
}

async fn harness_with(policy: FaultPolicy, secret: &str, configuration_set: &str) -> Harness {
    install_crypto_provider();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Endpoint {
        receiver: Arc::new(OnceReceiver::default()),
        seen: Arc::new(Mutex::new(Vec::new())),
        fail_first: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/dlr/aws_sms", post(endpoint))
        .with_state(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let mut config = FakeAwsConfig::with_defaults(format!("http://{addr}/dlr/aws_sms"));
    config.redelivery = Redelivery {
        attempts: 3,
        interval: Duration::from_millis(100),
    };
    let fake = FakeAws::start(config, policy).await;
    let receiver = AwsSmsProvider::receiver_with(
        fake.topic_arn().to_owned(),
        Arc::new(RedirectSnsHttp::new(fake.base_url()).unwrap()),
    )
    .unwrap();
    assert!(state.receiver.0.set(receiver).is_ok());
    let sender = AwsSmsProvider::sender(sender_config(&fake, secret, configuration_set)).unwrap();
    Harness {
        fake,
        sender,
        seen: state.seen,
        fail_first: state.fail_first,
    }
}

async fn harness(policy: FaultPolicy) -> Harness {
    harness_with(
        policy,
        sms_fake_aws::DEFAULT_SECRET_ACCESS_KEY,
        sms_fake_aws::DEFAULT_CONFIGURATION_SET,
    )
    .await
}

fn request() -> SubmitRequest {
    SubmitRequest {
        to: "+237677123456".to_owned(),
        sender_id: "VYMALO".to_owned(),
        body: "Votre code: 123456".to_owned(),
        encoding: SmsEncoding::Gsm7,
        reference: "c0fakeaws01".to_owned(),
    }
}

/// Waits until every scheduled notification has finished.
async fn settle(fake: &FakeAws) {
    for _ in 0..200 {
        if fake.ledger().notifications_in_flight() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("notifications still in flight after 5s");
}

#[tokio::test]
async fn an_accepted_message_is_reported_signed_and_authenticated() {
    let h = harness(FaultPolicy::always(SubmitDecision::delivered_after(
        Duration::from_millis(200),
    )))
    .await;

    let ack = h.sender.submit(&request()).await.expect("the fake accepts");
    settle(&h.fake).await;

    let submits = h.fake.ledger().submits();
    assert_eq!(submits.len(), 1);
    let submit = &submits[0];
    assert_eq!(
        submit.status, 200,
        "the fake's own Signature Version 4 check passed"
    );
    assert_eq!(
        submit.message_id.as_deref(),
        Some(ack.provider_ref.as_str())
    );
    assert_eq!(submit.destination, "+237677123456");
    assert_eq!(submit.origination_identity.as_deref(), Some("VYMALO"));
    assert_eq!(submit.configuration_set.as_deref(), Some("vsms-fake"));
    assert_eq!(submit.time_to_live, Some(900));
    assert_eq!(
        submit.context.get("vsmsMessageId").map(String::as_str),
        Some("c0fakeaws01")
    );

    let seen = h.seen.lock().unwrap().clone();
    let outcomes: Vec<_> = seen
        .iter()
        .map(|seen| match seen {
            Seen::Ingested(updates) => updates.clone(),
            other => panic!("expected an authentic receipt, got {other:?}"),
        })
        .collect();
    assert_eq!(
        outcomes,
        vec![
            vec![(ack.provider_ref.clone(), DeliveryOutcome::InFlight)],
            vec![(ack.provider_ref, DeliveryOutcome::Delivered)],
        ]
    );
    assert!(
        h.fake
            .ledger()
            .notifications()
            .iter()
            .all(|n| n.acknowledged && n.attempts == 1)
    );
}

#[tokio::test]
async fn a_wrong_secret_is_refused_by_the_fake_as_aws_would() {
    let h = harness_with(
        FaultPolicy::always(SubmitDecision::delivered_after(Duration::from_millis(50))),
        "not-the-secret",
        sms_fake_aws::DEFAULT_CONFIGURATION_SET,
    )
    .await;
    let error = h.sender.submit(&request()).await.unwrap_err();
    assert!(
        matches!(&error, ProviderError::Unavailable { message, .. } if message.contains("InvalidSignatureException")),
        "{error:?}"
    );
    assert_eq!(h.fake.ledger().submits()[0].status, 403);
    assert!(h.fake.ledger().accepted().is_empty());
}

#[tokio::test]
async fn a_missing_configuration_set_counts_against_the_account() {
    let h = harness_with(
        FaultPolicy::always(SubmitDecision::delivered_after(Duration::from_millis(50))),
        sms_fake_aws::DEFAULT_SECRET_ACCESS_KEY,
        "no-such-set",
    )
    .await;
    let error = h.sender.submit(&request()).await.unwrap_err();
    assert!(
        matches!(&error, ProviderError::Unavailable { message, .. } if message.contains("ResourceNotFoundException")),
        "{error:?}"
    );
}

#[tokio::test]
async fn the_sandbox_refusal_fails_over() {
    let h = harness(FaultPolicy::always(SubmitDecision::sandbox_unverified())).await;
    let error = h.sender.submit(&request()).await.unwrap_err();
    assert!(
        matches!(&error, ProviderError::Permanent { code, .. } if code == "DESTINATION_PHONE_NUMBER_NOT_VERIFIED"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_throttle_is_transient() {
    let h = harness(FaultPolicy::always(SubmitDecision::throttled())).await;
    assert!(matches!(
        h.sender.submit(&request()).await.unwrap_err(),
        ProviderError::Transient { .. }
    ));
}

#[tokio::test]
async fn an_accept_without_a_message_id_is_indeterminate() {
    let h = harness(FaultPolicy::always(SubmitDecision::without_message_id(
        Vec::new(),
    )))
    .await;
    assert!(matches!(
        h.sender.submit(&request()).await.unwrap_err(),
        ProviderError::Indeterminate { .. }
    ));
    assert_eq!(h.fake.ledger().accepted().len(), 1, "AWS has the message");
}

#[tokio::test]
async fn an_answer_slower_than_the_timeout_is_indeterminate_and_the_message_still_goes() {
    let h = harness(FaultPolicy::always(
        SubmitDecision::accepted_with_events(vec![EventStep::delivered(Duration::from_millis(50))])
            .delayed(Duration::from_secs(2)),
    ))
    .await;
    assert!(matches!(
        h.sender.submit(&request()).await.unwrap_err(),
        ProviderError::Indeterminate { .. }
    ));
    settle(&h.fake).await;
    assert_eq!(h.fake.ledger().accepted().len(), 1);
    assert_eq!(h.seen.lock().unwrap().len(), 1, "the receipt still arrives");
}

#[tokio::test]
async fn a_tampered_receipt_is_refused() {
    let h = harness(FaultPolicy::always(SubmitDecision::accepted_silently())).await;
    let genuine = h
        .fake
        .signed_notification(r#"{"eventType":"TEXT_DELIVERED","messageId":"a","isFinal":true}"#);
    let tampered = genuine.replacen("\\\"a\\\"", "\\\"b\\\"", 1);
    assert_ne!(
        genuine, tampered,
        "the test must actually change the signed Message"
    );

    let receiver = AwsSmsProvider::receiver_with(
        h.fake.topic_arn().to_owned(),
        Arc::new(RedirectSnsHttp::new(h.fake.base_url()).unwrap()),
    )
    .unwrap();
    let raw = |body: String| RawCallback {
        headers: Vec::new(),
        body: body.into_bytes(),
    };
    assert_eq!(
        receiver.authenticate_callback(&raw(genuine)).await.unwrap(),
        CallbackDisposition::Ingest
    );
    assert!(matches!(
        receiver
            .authenticate_callback(&raw(tampered))
            .await
            .unwrap(),
        CallbackDisposition::Refused { .. }
    ));
}

/// Records the URLs a receiver tries to fetch and serves nothing.
#[derive(Default)]
struct Recording(Mutex<Vec<String>>);

#[async_trait]
impl SnsHttp for Recording {
    async fn get(&self, url: &Url) -> Result<Vec<u8>, String> {
        self.0.lock().unwrap().push(url.to_string());
        Err("not served".to_owned())
    }
}

/// Without `RedirectSnsHttp` a receiver fetches the certificate from AWS's
/// own host, where the fake's key does not exist: nothing about a fake
/// receipt is trusted by default.
#[tokio::test]
async fn without_the_redirect_the_certificate_is_sought_on_aws_not_the_fake() {
    let h = harness(FaultPolicy::always(SubmitDecision::accepted_silently())).await;
    let http = Arc::new(Recording::default());
    let receiver =
        AwsSmsProvider::receiver_with(h.fake.topic_arn().to_owned(), http.clone()).unwrap();
    let raw = RawCallback {
        headers: Vec::new(),
        body: h
            .fake
            .signed_notification(r#"{"eventType":"TEXT_DELIVERED","messageId":"a"}"#)
            .into_bytes(),
    };
    assert!(receiver.authenticate_callback(&raw).await.is_err());
    let fetched = http.0.lock().unwrap().clone();
    assert_eq!(fetched, vec![h.fake.signing_cert_url()]);
    assert!(
        fetched[0].starts_with("https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-")
    );
}

#[tokio::test]
async fn an_unacknowledged_receipt_is_redelivered() {
    let h = harness(FaultPolicy::always(SubmitDecision::accepted_with_events(
        vec![EventStep::delivered(Duration::from_millis(20))],
    )))
    .await;
    h.fail_first
        .lock()
        .unwrap()
        .push(StatusCode::SERVICE_UNAVAILABLE);
    h.sender.submit(&request()).await.unwrap();
    settle(&h.fake).await;
    let notifications = h.fake.ledger().notifications();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].attempts, 2);
    assert!(notifications[0].acknowledged);
}

#[tokio::test]
async fn a_subscription_confirmation_is_acknowledged_not_ingested() {
    let h = harness(FaultPolicy::always(SubmitDecision::accepted_silently())).await;
    assert_eq!(h.fake.send_subscription_confirmation().await.unwrap(), 200);
    assert!(matches!(
        h.seen.lock().unwrap().as_slice(),
        [Seen::Acknowledged]
    ));
}

#[tokio::test]
async fn an_exhausted_script_is_loud() {
    let h = harness(FaultPolicy::scripted([SubmitDecision::accepted_silently()])).await;
    h.sender.submit(&request()).await.unwrap();
    assert!(matches!(
        h.sender.submit(&request()).await.unwrap_err(),
        ProviderError::Unavailable { .. }
    ));
    assert_eq!(h.fake.ledger().submits()[1].status, 500);
}
