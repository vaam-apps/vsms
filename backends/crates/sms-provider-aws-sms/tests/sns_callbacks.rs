//! The receiving half, end to end through the public API: an SNS delivery
//! is authenticated with `authenticate_callback` and then parsed with
//! `parse_dlr`, exactly the order `sms-gateway`'s `/dlr/{providerKey}`
//! route calls them in.
//!
//! The positive fixtures are signed by `openssl` over a string built by a
//! separate implementation (see `tests/fixtures/README.md`), so these tests
//! check the verifier against a third tool, not against itself. Every
//! negative case is the genuine fixture with one field changed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use sms_provider::{CallbackDisposition, DeliveryOutcome, ProviderError, RawCallback, SmsProvider};
use sms_provider_aws_sms::{AwsSmsProvider, SnsHttp};
use url::Url;

const TOPIC: &str = "arn:aws:sns:eu-west-1:123456789012:vsms-sms-events";
const CERT_URL: &str = "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-00000000000000000000000000test.pem";
const CERT: &[u8] = include_bytes!("fixtures/test-sns-signing-cert.pem");
const NOTIFICATION: &str = include_str!("fixtures/sns-notification-delivered.json");
const CONFIRMATION: &str = include_str!("fixtures/sns-subscription-confirmation.json");

/// Serves the fixture certificate at its URL, answers every other URL with
/// whatever `others` says, and records every GET.
#[derive(Default)]
struct FakeSns {
    others: HashMap<String, Result<Vec<u8>, String>>,
    cert_unavailable: bool,
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl SnsHttp for FakeSns {
    async fn get(&self, url: &Url) -> Result<Vec<u8>, String> {
        self.calls.lock().unwrap().push(url.to_string());
        if url.as_str() == CERT_URL {
            if self.cert_unavailable {
                return Err("connection reset".to_owned());
            }
            return Ok(CERT.to_vec());
        }
        self.others
            .get(url.as_str())
            .cloned()
            .unwrap_or_else(|| Err(format!("no fixture for {url}")))
    }
}

fn receiver(fake: Arc<FakeSns>) -> AwsSmsProvider {
    AwsSmsProvider::receiver_with(TOPIC.to_owned(), fake).expect("the fixture topic ARN is valid")
}

fn raw(body: &str) -> RawCallback {
    RawCallback {
        headers: vec![(
            "x-amz-sns-message-type".to_owned(),
            "Notification".to_owned(),
        )],
        body: body.as_bytes().to_vec(),
    }
}

/// The fixture with one top-level field replaced.
fn with(fixture: &str, field: &str, value: &str) -> String {
    let mut json: Value = serde_json::from_str(fixture).unwrap();
    json[field] = Value::String(value.to_owned());
    json.to_string()
}

fn refused(disposition: &CallbackDisposition) -> &str {
    match disposition {
        CallbackDisposition::Refused { reason } => reason,
        other => panic!("expected Refused, got {other:?}"),
    }
}

#[tokio::test]
async fn a_genuine_notification_is_ingested_and_parses_to_delivered() {
    let fake = Arc::new(FakeSns::default());
    let provider = receiver(fake.clone());

    let disposition = provider
        .authenticate_callback(&raw(NOTIFICATION))
        .await
        .unwrap();
    assert_eq!(disposition, CallbackDisposition::Ingest);

    let updates = provider.parse_dlr(&raw(NOTIFICATION)).unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(
        updates[0].provider_ref,
        "5f1c0a9e-8a51-4d0b-9c1e-6a3c9b2d7e41"
    );
    assert_eq!(updates[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(updates[0].raw_status, "TEXT_DELIVERED");
}

#[tokio::test]
async fn the_signing_certificate_is_fetched_once_and_cached() {
    let fake = Arc::new(FakeSns::default());
    let provider = receiver(fake.clone());
    for _ in 0..3 {
        provider
            .authenticate_callback(&raw(NOTIFICATION))
            .await
            .unwrap();
    }
    assert_eq!(fake.calls.lock().unwrap().as_slice(), [CERT_URL]);
}

/// The one-byte forgery: a valid envelope whose `Message` says something
/// SNS never signed.
#[tokio::test]
async fn a_tampered_message_is_refused() {
    let json: Value = serde_json::from_str(NOTIFICATION).unwrap();
    let forged_message = json["Message"]
        .as_str()
        .unwrap()
        .replace("TEXT_DELIVERED", "TEXT_DELIVERE_");
    let forged = with(NOTIFICATION, "Message", &forged_message);

    let disposition = receiver(Arc::new(FakeSns::default()))
        .authenticate_callback(&raw(&forged))
        .await
        .unwrap();
    assert!(
        refused(&disposition).contains("does not verify"),
        "{disposition:?}"
    );
}

/// A message signed by SNS for someone else's topic. The check that
/// matters most (see `sns.md`), and it must run before anything is fetched.
#[tokio::test]
async fn another_topic_is_refused_before_any_fetch() {
    let fake = Arc::new(FakeSns::default());
    let forged = with(
        NOTIFICATION,
        "TopicArn",
        "arn:aws:sns:eu-west-1:999999999999:attacker",
    );

    let disposition = receiver(fake.clone())
        .authenticate_callback(&raw(&forged))
        .await
        .unwrap();
    assert!(
        refused(&disposition).contains("TopicArn"),
        "{disposition:?}"
    );
    assert!(
        fake.calls.lock().unwrap().is_empty(),
        "nothing may be fetched for a foreign topic"
    );
}

#[tokio::test]
async fn signature_version_1_is_refused() {
    let forged = with(NOTIFICATION, "SignatureVersion", "1");
    let disposition = receiver(Arc::new(FakeSns::default()))
        .authenticate_callback(&raw(&forged))
        .await
        .unwrap();
    assert!(
        refused(&disposition).contains("SignatureVersion"),
        "{disposition:?}"
    );
}

#[tokio::test]
async fn a_certificate_url_off_the_topics_sns_host_is_refused_without_fetching() {
    for url in [
        "https://evil.example/SimpleNotificationService-x.pem",
        "https://sns.us-east-1.amazonaws.com/SimpleNotificationService-x.pem",
        "http://sns.eu-west-1.amazonaws.com/SimpleNotificationService-x.pem",
        "https://sns.eu-west-1.amazonaws.com/not-a-certificate.txt",
    ] {
        let fake = Arc::new(FakeSns::default());
        let forged = with(NOTIFICATION, "SigningCertURL", url);
        let disposition = receiver(fake.clone())
            .authenticate_callback(&raw(&forged))
            .await
            .unwrap();
        assert!(
            refused(&disposition).contains("SigningCertURL"),
            "{url}: {disposition:?}"
        );
        assert!(
            fake.calls.lock().unwrap().is_empty(),
            "{url} must not be fetched"
        );
    }
}

#[tokio::test]
async fn a_body_that_is_not_an_sns_message_is_refused() {
    let disposition = receiver(Arc::new(FakeSns::default()))
        .authenticate_callback(&raw(r#"{"eventType":"TEXT_DELIVERED","messageId":"x"}"#))
        .await
        .unwrap();
    assert!(
        refused(&disposition).contains("not an SNS message"),
        "{disposition:?}"
    );
}

/// The check could not run: retryable, not a refusal, so SNS redelivers a
/// genuine receipt instead of the gateway discarding it.
#[tokio::test]
async fn an_unreachable_certificate_is_an_error_not_a_refusal() {
    let fake = Arc::new(FakeSns {
        cert_unavailable: true,
        ..FakeSns::default()
    });
    let result = receiver(fake)
        .authenticate_callback(&raw(NOTIFICATION))
        .await;
    assert!(
        matches!(result, Err(ProviderError::Unavailable { .. })),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_genuine_subscription_confirmation_is_confirmed_by_visiting_its_subscribe_url() {
    let json: Value = serde_json::from_str(CONFIRMATION).unwrap();
    let subscribe_url = json["SubscribeURL"].as_str().unwrap().to_owned();
    let fake = Arc::new(FakeSns {
        others: HashMap::from([(
            subscribe_url.clone(),
            Ok(b"<ConfirmSubscriptionResponse/>".to_vec()),
        )]),
        ..FakeSns::default()
    });
    let provider = receiver(fake.clone());

    let disposition = provider
        .authenticate_callback(&raw(CONFIRMATION))
        .await
        .unwrap();
    assert!(
        matches!(disposition, CallbackDisposition::Acknowledged { .. }),
        "{disposition:?}"
    );
    let calls = fake.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        [
            CERT_URL.to_owned(),
            Url::parse(&subscribe_url).unwrap().to_string()
        ]
    );

    // A handshake carries no delivery update.
    assert!(provider.parse_dlr(&raw(CONFIRMATION)).unwrap().is_empty());
}

/// A confirmation whose signature does not verify (here: a `SubscribeURL`
/// pointing somewhere else) is never visited.
#[tokio::test]
async fn a_forged_subscription_confirmation_is_never_visited() {
    let fake = Arc::new(FakeSns::default());
    let forged = with(
        CONFIRMATION,
        "SubscribeURL",
        "https://sns.eu-west-1.amazonaws.com/?Action=ConfirmSubscription&TopicArn=arn:aws:sns:eu-west-1:999999999999:attacker&Token=x",
    );
    let disposition = receiver(fake.clone())
        .authenticate_callback(&raw(&forged))
        .await
        .unwrap();
    refused(&disposition);
    assert_eq!(
        fake.calls.lock().unwrap().as_slice(),
        [CERT_URL],
        "only the certificate is fetched"
    );
}

#[tokio::test]
async fn a_sender_refuses_every_callback() {
    let sender = AwsSmsProvider::sender(sms_provider_aws_sms::AwsSmsConfig {
        region: "eu-west-1".to_owned(),
        access_key_id: "AKIDEXAMPLE".to_owned(),
        secret_access_key: "secret".to_owned(),
        configuration_set: "vsms-events".to_owned(),
        origination_identity: None,
        message_type: sms_provider_aws_sms::MessageType::Transactional,
        tps_ceiling: 1.0,
        cost_per_segment_xaf: rust_decimal::Decimal::ONE,
        supports_alphanumeric_sender: true,
        endpoint_url: None,
        connect_timeout: std::time::Duration::from_secs(1),
        request_timeout: std::time::Duration::from_secs(1),
    })
    .unwrap();
    let disposition = sender
        .authenticate_callback(&raw(NOTIFICATION))
        .await
        .unwrap();
    refused(&disposition);
}

#[test]
fn a_receiver_needs_a_real_sns_topic_arn() {
    for bad in [
        "",
        "vsms-sms-events",
        "arn:aws:sqs:eu-west-1:1:q",
        "arn:aws-cn:sns:cn-north-1:1:t",
    ] {
        assert!(AwsSmsProvider::receiver(bad.to_owned()).is_err(), "{bad:?}");
    }
}
