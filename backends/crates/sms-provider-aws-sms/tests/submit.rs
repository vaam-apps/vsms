//! The sending half against a `wiremock` stand-in for
//! `sms-voice.<region>.amazonaws.com`. The wire contract asserted here is
//! botocore's `pinpoint-sms-voice-v2` model; the signature itself is
//! checked against fixed vectors in `src/sigv4.rs`.

use std::time::Duration;

use rust_decimal::Decimal;
use serde_json::{Value, json};
use sms_encoding::SmsEncoding;
use sms_provider::{ProviderError, SmsProvider, SubmitRequest};
use sms_provider_aws_sms::{AwsSmsConfig, AwsSmsProvider, MessageType};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(endpoint: &str) -> AwsSmsConfig {
    AwsSmsConfig {
        region: "eu-west-1".to_owned(),
        access_key_id: "AKIDEXAMPLE".to_owned(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_owned(),
        configuration_set: "vsms-events".to_owned(),
        origination_identity: None,
        message_type: MessageType::Transactional,
        tps_ceiling: 20.0,
        cost_per_segment_xaf: Decimal::new(30, 0),
        supports_alphanumeric_sender: true,
        endpoint_url: Some(endpoint.to_owned()),
        time_to_live: None,
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(2),
    }
}

fn request() -> SubmitRequest {
    SubmitRequest {
        to: "+237677123456".to_owned(),
        sender_id: "VYMALO".to_owned(),
        body: "Votre code: 123456".to_owned(),
        encoding: SmsEncoding::Gsm7,
        reference: "c0abc123".to_owned(),
    }
}

async fn send(config: AwsSmsConfig) -> Result<sms_provider::SubmitAck, ProviderError> {
    AwsSmsProvider::sender(config)
        .unwrap()
        .submit(&request())
        .await
}

#[tokio::test]
async fn a_successful_send_returns_the_message_id_and_sends_the_documented_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("x-amz-target", "PinpointSMSVoiceV2.SendTextMessage"))
        .and(header("content-type", "application/x-amz-json-1.0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MessageId": "aws-msg-1"})))
        .expect(1)
        .mount(&server)
        .await;

    let ack = send(config(&server.uri())).await.unwrap();
    assert_eq!(ack.provider_ref, "aws-msg-1");
    assert_eq!(ack.provider_ref_alt, None);

    let received = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&received.body).unwrap();
    assert_eq!(
        body,
        json!({
            "DestinationPhoneNumber": "+237677123456",
            "OriginationIdentity": "VYMALO",
            "MessageBody": "Votre code: 123456",
            "MessageType": "TRANSACTIONAL",
            "ConfigurationSetName": "vsms-events",
            "Context": {"vsmsMessageId": "c0abc123"},
        })
    );

    let authorization = received
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    let date = received
        .headers
        .get("x-amz-date")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(date.len(), 16, "x-amz-date is YYYYMMDDTHHMMSSZ: {date}");
    assert!(
        authorization.starts_with(&format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/{}/eu-west-1/sms-voice/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date;x-amz-target, Signature=",
            &date[..8]
        )),
        "{authorization}"
    );
    assert!(
        !String::from_utf8_lossy(&received.body).contains("wJalrXUtnFEMI"),
        "the secret key must never be sent"
    );
}

#[tokio::test]
async fn a_time_to_live_is_sent_in_seconds_and_omitted_when_unset() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MessageId": "m"})))
        .mount(&server)
        .await;

    let mut with_ttl = config(&server.uri());
    with_ttl.time_to_live = Some(Duration::from_mins(15));
    send(with_ttl).await.unwrap();
    send(config(&server.uri())).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(first["TimeToLive"], 900);
    assert!(second.get("TimeToLive").is_none(), "{second}");
}

#[test]
fn a_time_to_live_outside_what_aws_accepts_is_refused_at_construction() {
    for secs in [0, 4, 259_201] {
        let mut config = config("https://sms-voice.eu-west-1.amazonaws.com/");
        config.time_to_live = Some(Duration::from_secs(secs));
        assert!(
            matches!(
                AwsSmsProvider::sender(config),
                Err(sms_provider_aws_sms::ConfigError::TimeToLive(_))
            ),
            "{secs}"
        );
    }
    for secs in [5, 259_200] {
        let mut config = config("https://sms-voice.eu-west-1.amazonaws.com/");
        config.time_to_live = Some(Duration::from_secs(secs));
        assert!(AwsSmsProvider::sender(config).is_ok(), "{secs}");
    }
}

/// An empty environment variable reaches the config as `Some("")`.
#[tokio::test]
async fn blank_optional_values_are_treated_as_unset() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MessageId": "m"})))
        .mount(&server)
        .await;
    let mut blank_identity = config(&server.uri());
    blank_identity.origination_identity = Some("  ".to_owned());
    send(blank_identity).await.unwrap();
    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(
        body["OriginationIdentity"], "VYMALO",
        "a blank override must fall back to the message's own sender"
    );

    // A blank endpoint override falls back to the regional endpoint
    // rather than failing construction as an unusable URL.
    let mut blank_endpoint = config("");
    blank_endpoint.endpoint_url = Some(String::new());
    assert!(AwsSmsProvider::sender(blank_endpoint).is_ok());
}

#[tokio::test]
async fn a_configured_origination_identity_overrides_the_messages_sender() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MessageId": "m"})))
        .mount(&server)
        .await;
    let mut config = config(&server.uri());
    config.origination_identity =
        Some("arn:aws:sms-voice:eu-west-1:123456789012:pool/pool-abc".to_owned());

    send(config).await.unwrap();
    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(
        body["OriginationIdentity"],
        "arn:aws:sms-voice:eu-west-1:123456789012:pool/pool-abc"
    );
}

/// AWS took the message; we just could not read what it called it.
#[tokio::test]
async fn a_200_without_a_message_id_is_indeterminate_not_retryable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    let error = send(config(&server.uri())).await.unwrap_err();
    assert!(
        matches!(error, ProviderError::Indeterminate { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_post_connect_timeout_is_indeterminate() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"MessageId": "late"}))
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    let mut config = config(&server.uri());
    config.request_timeout = Duration::from_millis(50);
    let error = send(config).await.unwrap_err();
    assert!(
        matches!(error, ProviderError::Indeterminate { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_connect_refusal_is_unavailable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let error = AwsSmsProvider::sender(config(&format!("http://{addr}/")))
        .unwrap()
        .submit(&request())
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProviderError::Unavailable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn throttling_is_transient() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400)
                .insert_header("x-amzn-ErrorType", "ThrottlingException")
                .set_body_json(
                    json!({"__type": "ThrottlingException", "message": "Rate exceeded"}),
                ),
        )
        .mount(&server)
        .await;
    let error = send(config(&server.uri())).await.unwrap_err();
    assert!(
        matches!(error, ProviderError::Transient { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_reached_spend_limit_is_unavailable_so_routing_fails_over() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "__type": "ServiceQuotaExceededException",
            "Message": "Monthly spend limit reached",
            "Reason": "MONTHLY_SPEND_LIMIT_REACHED_FOR_TEXT"
        })))
        .mount(&server)
        .await;
    let error = send(config(&server.uri())).await.unwrap_err();
    match error {
        ProviderError::Unavailable { message, .. } => {
            assert!(
                message.contains("MONTHLY_SPEND_LIMIT_REACHED_FOR_TEXT"),
                "{message}"
            );
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

/// A message's own sender ID that AWS does not know fails that sender on
/// this route; it must not open the provider breaker for everyone. A
/// configured identity is account configuration, so the same error there
/// is account-level.
#[tokio::test]
async fn an_unknown_request_sender_is_permanent_but_an_unknown_configured_identity_is_not() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "__type": "ResourceNotFoundException",
            "Message": "Sender ID not found",
            "ResourceType": "sender-id",
            "ResourceId": "VYMALO"
        })))
        .mount(&server)
        .await;

    let error = send(config(&server.uri())).await.unwrap_err();
    assert!(
        matches!(error, ProviderError::Permanent { .. }),
        "{error:?}"
    );

    let mut configured = config(&server.uri());
    configured.origination_identity = Some("pool-abc".to_owned());
    let error = send(configured).await.unwrap_err();
    assert!(
        matches!(error, ProviderError::Unavailable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_unregistered_sender_tries_the_next_route() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "__type": "ValidationException",
            "Message": "Sender ID requires registration",
            "Reason": "SENDER_ID_REQUIRES_REGISTRATION"
        })))
        .mount(&server)
        .await;
    let error = send(config(&server.uri())).await.unwrap_err();
    assert!(
        matches!(error, ProviderError::Permanent { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_body_over_aws_limit_is_rejected_without_a_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let mut long = request();
    long.body = "a".repeat(1601);
    let error = AwsSmsProvider::sender(config(&server.uri()))
        .unwrap()
        .submit(&long)
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::Rejected { code, .. } if code == "BODY_TOO_LONG"));
}

#[tokio::test]
async fn a_receiver_cannot_send() {
    let receiver =
        AwsSmsProvider::receiver("arn:aws:sns:eu-west-1:123456789012:vsms-sms-events".to_owned())
            .unwrap();
    let error = receiver.submit(&request()).await.unwrap_err();
    assert!(
        matches!(error, ProviderError::Unavailable { .. }),
        "{error:?}"
    );
    assert!(receiver.capabilities().tps_ceiling.abs() < f64::EPSILON);
}

#[test]
fn a_config_that_could_never_send_is_refused_at_construction() {
    let mut bad_region = config("https://sms-voice.eu-west-1.amazonaws.com/");
    bad_region.region = "EU_WEST".to_owned();
    assert!(AwsSmsProvider::sender(bad_region).is_err());

    let mut no_config_set = config("https://sms-voice.eu-west-1.amazonaws.com/");
    no_config_set.configuration_set = "  ".to_owned();
    assert!(AwsSmsProvider::sender(no_config_set).is_err());

    assert!(AwsSmsProvider::sender(config("not a url")).is_err());
    assert!(AwsSmsProvider::sender(config("ftp://example.com/")).is_err());
}

#[test]
fn debug_never_prints_the_secret() {
    let rendered = format!("{:?}", config("https://sms-voice.eu-west-1.amazonaws.com/"));
    assert!(!rendered.contains("wJalrXUtnFEMI"), "{rendered}");
    assert!(rendered.contains("<redacted>"));
}
