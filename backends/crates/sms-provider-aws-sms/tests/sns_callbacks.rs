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
const CERT_URL: &str = "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-00000000000000000000000000c0ffee.pem";
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
    assert_eq!(
        fake.calls.lock().unwrap().len(),
        0,
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
        assert_cert_url_refused(url).await;
    }
}

/// Bounds what a forger can make the gateway fetch: only
/// `/SimpleNotificationService-<32 hex>.pem` on the region's SNS host,
/// nothing after the path.
#[tokio::test]
async fn a_certificate_url_that_is_not_exactly_an_sns_signing_certificate_is_refused() {
    for url in [
        // Right host and extension, wrong path.
        "https://sns.eu-west-1.amazonaws.com/other/SimpleNotificationService-0123456789abcdef0123456789abcdef.pem",
        "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-0123456789abcdef0123456789abcdef.pem/extra",
        "https://sns.eu-west-1.amazonaws.com/Evil-0123456789abcdef0123456789abcdef.pem",
        "https://sns.eu-west-1.amazonaws.com/",
        "https://sns.eu-west-1.amazonaws.com/x.pem",
        // Not 32 hex digits.
        "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-0123456789abcdef0123456789abcde.pem",
        "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-0123456789abcdef0123456789abcdef0.pem",
        "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-0123456789abcdef0123456789abcdeg.pem",
        // Query and fragment.
        "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-0123456789abcdef0123456789abcdef.pem?x=1",
        "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-0123456789abcdef0123456789abcdef.pem#frag",
    ] {
        assert_cert_url_refused(url).await;
    }
}

async fn assert_cert_url_refused(url: &str) {
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
    assert_eq!(
        fake.calls.lock().unwrap().len(),
        0,
        "{url} must not be fetched"
    );
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

/// Confirming by GET leaves `AuthenticateOnUnsubscribe` false, so an
/// authentic confirmation is acknowledged and left for the operator: the
/// only request made is the certificate fetch.
#[tokio::test]
async fn a_genuine_subscription_confirmation_is_acknowledged_but_never_visited() {
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
        matches!(&disposition, CallbackDisposition::Acknowledged { detail } if detail.contains("not confirmed")),
        "{disposition:?}"
    );
    assert_eq!(
        fake.calls.lock().unwrap().as_slice(),
        [CERT_URL],
        "the SubscribeURL must not be visited"
    );

    // A handshake carries no delivery update.
    assert_eq!(provider.parse_dlr(&raw(CONFIRMATION)).unwrap().len(), 0);
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

/// A forger cannot make the gateway fetch more than one uncached
/// certificate per interval, however many distinct valid-looking URLs
/// it signs messages with.
#[tokio::test]
async fn uncached_certificate_fetches_are_rate_limited() {
    let fake = Arc::new(FakeSns::default());
    let provider = receiver(fake.clone());
    let url = |n: u8| {
        format!("https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-{n:032x}.pem")
    };

    // The first miss is fetched (and fails: the fake has no such file).
    let first = provider
        .authenticate_callback(&raw(&with(NOTIFICATION, "SigningCertURL", &url(1))))
        .await;
    assert!(matches!(first, Err(ProviderError::Unavailable { .. })));

    // A second, different miss straight after is refused without a fetch.
    let second = provider
        .authenticate_callback(&raw(&with(NOTIFICATION, "SigningCertURL", &url(2))))
        .await;
    assert!(
        matches!(&second, Err(ProviderError::Unavailable { message, .. }) if message.contains("rate limited")),
        "{second:?}"
    );
    assert_eq!(fake.calls.lock().unwrap().len(), 1, "{:?}", fake.calls);
}

/// Hex digits are case-insensitive in the certificate's file name.
#[tokio::test]
async fn an_uppercase_hex_certificate_name_is_accepted_for_fetching() {
    let fake = Arc::new(FakeSns::default());
    let url = "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-0123456789ABCDEF0123456789ABCDEF.pem";
    let result = receiver(fake.clone())
        .authenticate_callback(&raw(&with(NOTIFICATION, "SigningCertURL", url)))
        .await;
    // Fetched (the fake has no such file, so Unavailable), not Refused.
    assert!(matches!(result, Err(ProviderError::Unavailable { .. })));
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
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
        time_to_live: None,
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
