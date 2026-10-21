#![doc = include_str!("lib.md")]

mod events;
mod sigv4;
mod sns;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sms_provider::{
    CallbackDisposition, Capabilities, DeliveryUpdate, Health, ProviderError, RawCallback,
    SmsProvider, SubmitAck, SubmitRequest,
};
use sms_provider_http::{classify_common_submit_status, classify_transport_error};
use url::Url;

pub use sns::{SnsHttp, region_of_topic_arn};

/// `Provider.key` for this adapter. Market-agnostic, unlike `orange_cm` /
/// `mtn_cm`: AWS End User Messaging is one account reaching every country
/// it supports, so the key names the service rather than a market.
pub const KEY: &str = "aws_sms";

/// The noun transport-error messages name this provider as.
const PROVIDER_NOUN: &str = "AWS";

/// The service's signing name and endpoint prefix (`sms-voice`), from
/// botocore's `pinpoint-sms-voice-v2` service model.
const SERVICE: &str = "sms-voice";

/// AWS JSON 1.0 protocol: one POST to `/`, the operation named in
/// `X-Amz-Target` as `<targetPrefix>.<operation>`.
const TARGET: &str = "PinpointSMSVoiceV2.SendTextMessage";
const JSON_1_0: &str = "application/x-amz-json-1.0";

/// `SendTextMessage`'s own `MessageBody` maximum length (botocore model).
const MAX_BODY_CHARS: usize = 1600;

/// Retry delay after a `ThrottlingException`. AWS documents no
/// `Retry-After` for this API; one second matches the per-second
/// message-part quotas the throttle enforces.
const THROTTLE_RETRY_AFTER: Duration = Duration::from_secs(1);

/// The `Context` key `submit` stamps on every message with our own
/// `Message.id`. AWS echoes `context` back in every event record, so an
/// operator reading a raw receipt can always tie it to a message.
const CONTEXT_MESSAGE_ID: &str = "vsmsMessageId";

/// `SendTextMessage`'s `MessageType`. Carrier handling (and in some
/// countries, price and filtering) differs between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// OTPs and account notifications. The right default for this gateway.
    Transactional,
    /// Marketing.
    Promotional,
}

impl MessageType {
    const fn wire(self) -> &'static str {
        match self {
            Self::Transactional => "TRANSACTIONAL",
            Self::Promotional => "PROMOTIONAL",
        }
    }
}

impl std::str::FromStr for MessageType {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "transactional" => Ok(Self::Transactional),
            "promotional" => Ok(Self::Promotional),
            other => Err(format!("{other:?} is not transactional or promotional")),
        }
    }
}

/// What the sending side needs. No `Default`: every commercial and
/// account-specific value must be stated, never invented — see
/// `sms_provider_mtn::MtnConfig`'s identical reasoning.
#[derive(Clone)]
pub struct AwsSmsConfig {
    /// AWS region the account sends from, e.g. `eu-west-1`.
    pub region: String,
    /// Access key id of an IAM principal allowed `sms-voice:SendTextMessage`.
    pub access_key_id: String,
    /// The matching secret. Never logged — see the hand-written `Debug`.
    pub secret_access_key: String,
    /// The configuration set whose SNS event destination delivers receipts
    /// to `/dlr/aws_sms`. Required: without one, AWS sends no events and
    /// every message ends `expired` (see `lib.md`).
    pub configuration_set: String,
    /// A fixed origination identity (sender ID, phone number, pool, or one
    /// of their ARNs) used for every message. When `None`, the message's own
    /// approved sender ID is passed instead, which then has to exist in the
    /// AWS account too.
    pub origination_identity: Option<String>,
    /// Transactional or promotional.
    pub message_type: MessageType,
    /// Message parts per second this account may send. An account quota,
    /// so never defaulted.
    pub tps_ceiling: f64,
    /// What one segment costs, converted to XAF. AWS prices in USD per
    /// destination country; the conversion is the operator's to state.
    pub cost_per_segment_xaf: Decimal,
    /// Whether the account can send under an alphanumeric sender ID to the
    /// destinations this deployment serves.
    pub supports_alphanumeric_sender: bool,
    /// Override for the endpoint, for tests and for VPC endpoints. `None`
    /// is `https://sms-voice.<region>.amazonaws.com/`.
    pub endpoint_url: Option<String>,
    /// TCP/TLS connect timeout.
    pub connect_timeout: Duration,
    /// Whole-request timeout. A timeout after connecting is `Indeterminate`.
    pub request_timeout: Duration,
}

impl std::fmt::Debug for AwsSmsConfig {
    /// Hand-written so `secret_access_key` can never reach a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsSmsConfig")
            .field("region", &self.region)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field("configuration_set", &self.configuration_set)
            .field("origination_identity", &self.origination_identity)
            .field("message_type", &self.message_type)
            .field("tps_ceiling", &self.tps_ceiling)
            .field("cost_per_segment_xaf", &self.cost_per_segment_xaf)
            .field(
                "supports_alphanumeric_sender",
                &self.supports_alphanumeric_sender,
            )
            .field("endpoint_url", &self.endpoint_url)
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

/// A configuration that cannot work, found at construction rather than at
/// the first message.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The region is not shaped like one.
    #[error("{0:?} is not an AWS region name")]
    Region(String),
    /// The endpoint override is not an absolute http(s) URL.
    #[error("{0:?} is not a usable endpoint URL")]
    Endpoint(String),
    /// A required value is empty.
    #[error("{0} must not be empty")]
    Empty(&'static str),
    /// The events topic is not an `aws`-partition SNS topic ARN.
    #[error("{0:?} is not an SNS topic ARN (arn:aws:sns:<region>:<account>:<name>)")]
    TopicArn(String),
}

struct SendSide {
    config: AwsSmsConfig,
    endpoint: Url,
    /// `endpoint`'s authority exactly as it goes in the `Host` header.
    host: String,
}

/// AWS End User Messaging SMS. A process is either a sender (the worker)
/// or a receiver of delivery events (the gateway); see `lib.md`.
pub struct AwsSmsProvider {
    client: reqwest::Client,
    send: Option<SendSide>,
    verifier: Option<sns::Verifier>,
}

impl AwsSmsProvider {
    /// The sending side, for the worker.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] for a configuration that could never send.
    ///
    /// # Panics
    ///
    /// Never in practice: building a `reqwest` client with only timeouts
    /// set cannot fail.
    pub fn sender(config: AwsSmsConfig) -> Result<Self, ConfigError> {
        if !is_region(&config.region) {
            return Err(ConfigError::Region(config.region));
        }
        for (name, value) in [
            ("access key id", &config.access_key_id),
            ("secret access key", &config.secret_access_key),
            ("configuration set", &config.configuration_set),
        ] {
            if value.trim().is_empty() {
                return Err(ConfigError::Empty(name));
            }
        }
        let raw_endpoint = config
            .endpoint_url
            .clone()
            .unwrap_or_else(|| format!("https://{SERVICE}.{}.amazonaws.com/", config.region));
        let endpoint = Url::parse(&raw_endpoint)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https") && url.query().is_none())
            .ok_or_else(|| ConfigError::Endpoint(raw_endpoint.clone()))?;
        let host = endpoint.authority().to_owned();
        if host.is_empty() {
            return Err(ConfigError::Endpoint(raw_endpoint));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .build()
            .expect("reqwest client builder with only timeouts set never fails");
        Ok(Self {
            client,
            send: Some(SendSide {
                config,
                endpoint,
                host,
            }),
            verifier: None,
        })
    }

    /// The receiving side, for the gateway: authenticates SNS deliveries
    /// for `events_topic_arn` and parses the events they carry. Needs no
    /// AWS credentials at all.
    ///
    /// # Errors
    ///
    /// [`ConfigError::TopicArn`] when the ARN is not an SNS topic ARN.
    pub fn receiver(events_topic_arn: String) -> Result<Self, ConfigError> {
        Self::receiver_with(events_topic_arn, Arc::new(sns::ReqwestSnsHttp::new()))
    }

    /// [`Self::receiver`] with the HTTP client used for certificate fetches
    /// and subscription confirmation supplied by the caller — the seam
    /// tests use to serve a fixture certificate.
    ///
    /// # Errors
    ///
    /// As [`Self::receiver`].
    pub fn receiver_with(
        events_topic_arn: String,
        http: Arc<dyn SnsHttp>,
    ) -> Result<Self, ConfigError> {
        let verifier = sns::Verifier::new(events_topic_arn.clone(), http)
            .ok_or(ConfigError::TopicArn(events_topic_arn))?;
        Ok(Self {
            client: reqwest::Client::new(),
            send: None,
            verifier: Some(verifier),
        })
    }
}

/// Lowercase letters, digits and hyphens, at least one hyphen
/// (`eu-west-1`, `af-south-1`). Loose on purpose: AWS adds regions, and the
/// first real request is what proves one exists.
fn is_region(region: &str) -> bool {
    !region.is_empty()
        && region.contains('-')
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !region.starts_with('-')
        && !region.ends_with('-')
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SendTextMessageRequest<'a> {
    destination_phone_number: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    origination_identity: Option<&'a str>,
    message_body: &'a str,
    message_type: &'static str,
    configuration_set_name: &'a str,
    context: BTreeMap<&'static str, &'a str>,
}

#[derive(Deserialize)]
struct SendTextMessageResponse {
    #[serde(rename = "MessageId", default)]
    message_id: Option<String>,
}

#[derive(Deserialize, Default)]
struct AwsErrorBody {
    #[serde(rename = "__type", default)]
    kind: Option<String>,
    #[serde(alias = "Message", default)]
    message: Option<String>,
    #[serde(rename = "Reason", default)]
    reason: Option<String>,
}

/// `ValidationException` reasons that mean "AWS cannot carry *this*
/// message, but another route might": the destination country, the sender
/// identity, or the price — never the message itself. Everything else a
/// `ValidationException` reports is about the request, and fails anywhere.
const ROUTE_SPECIFIC_VALIDATION_REASONS: &[&str] = &[
    "CHANNEL_NOT_ENABLED",
    "COUNTRY_NOT_ENABLED",
    "DESTINATION_COUNTRY_BLOCKED",
    "INTERNATIONAL_SENDING_NOT_SUPPORTED",
    "INVALID_IDENTITY_FOR_DESTINATION_COUNTRY",
    "PRICE_OVER_THRESHOLD",
    "SENDER_ID_NOT_REGISTERED",
    "SENDER_ID_NOT_SUPPORTED",
    "SENDER_ID_REQUIRES_REGISTRATION",
];

/// Error types that mean this account cannot send right now, for any
/// message: credentials, account standing, a quota or spend limit, or a
/// configuration set / identity that does not exist. Each fails over and
/// counts toward the provider circuit breaker, and none of them sent
/// anything.
const ACCOUNT_LEVEL_ERRORS: &[&str] = &[
    "AccessDeniedException",
    "ExpiredTokenException",
    "IncompleteSignature",
    "InvalidClientTokenId",
    "InvalidSignatureException",
    "MissingAuthenticationToken",
    "ResourceNotFoundException",
    "ServiceQuotaExceededException",
    "SignatureDoesNotMatch",
    "UnrecognizedClientException",
];

/// Classify a non-2xx `SendTextMessage` response. `error_type` is the
/// `x-amzn-ErrorType` header when present; the body's `__type` otherwise.
fn classify_error(status: StatusCode, error_type: Option<&str>, body: &str) -> ProviderError {
    let parsed: AwsErrorBody = serde_json::from_str(body).unwrap_or_default();
    // Either source may be `com.amazonaws.…#ThrottlingException`, and the
    // header may carry a `:http://…` suffix; the bare name is what matters.
    let kind = error_type
        .map(str::to_owned)
        .or(parsed.kind)
        .map(|raw| {
            let name = raw.rsplit('#').next().unwrap_or(&raw);
            name.split(':').next().unwrap_or(name).to_owned()
        })
        .unwrap_or_default();
    let message = parsed.message.unwrap_or_else(|| body.to_owned());

    if kind == "ThrottlingException" {
        return ProviderError::Transient {
            retry_after: THROTTLE_RETRY_AFTER,
            message,
        };
    }
    if ACCOUNT_LEVEL_ERRORS.contains(&kind.as_str()) {
        let detail = parsed
            .reason
            .map(|reason| format!(" ({reason})"))
            .unwrap_or_default();
        return ProviderError::Unavailable {
            message: format!("AWS refused the account: {kind}{detail}: {message}"),
            source: None,
        };
    }
    if kind == "ValidationException" {
        let reason = parsed.reason.unwrap_or_else(|| kind.clone());
        if ROUTE_SPECIFIC_VALIDATION_REASONS.contains(&reason.as_str()) {
            return ProviderError::Permanent {
                code: reason,
                message,
            };
        }
        return ProviderError::Rejected {
            code: reason,
            message,
        };
    }
    if kind == "ConflictException" {
        // Most often the destination has opted out of this account
        // (replied STOP to an AWS number). Not retryable here.
        return ProviderError::Rejected {
            code: parsed.reason.unwrap_or(kind),
            message,
        };
    }
    classify_common_submit_status(status, &message, PROVIDER_NOUN, THROTTLE_RETRY_AFTER)
}

#[async_trait]
impl SmsProvider for AwsSmsProvider {
    // The trait fixes `&self -> &str`.
    #[allow(clippy::unnecessary_literal_bound)]
    fn key(&self) -> &str {
        KEY
    }

    fn capabilities(&self) -> Capabilities {
        match &self.send {
            Some(send) => Capabilities {
                dlr: true,
                alphanumeric_sender: send.config.supports_alphanumeric_sender,
                // AWS chooses GSM-7 or UCS-2 itself and splits up to
                // 1600 characters into parts.
                ucs2: true,
                concatenation: true,
                tps_ceiling: send.config.tps_ceiling,
                cost_per_segment_xaf: send.config.cost_per_segment_xaf,
            },
            // A receiver never sends; it contributes nothing to dispatch
            // throughput.
            None => Capabilities {
                dlr: true,
                alphanumeric_sender: false,
                ucs2: false,
                concatenation: false,
                tps_ceiling: 0.0,
                cost_per_segment_xaf: Decimal::ZERO,
            },
        }
    }

    async fn submit(&self, req: &SubmitRequest) -> Result<SubmitAck, ProviderError> {
        let Some(send) = &self.send else {
            return Err(ProviderError::Unavailable {
                message: "this AwsSmsProvider only receives delivery events; it has no sending credentials"
                    .to_owned(),
                source: None,
            });
        };
        if req.body.chars().count() > MAX_BODY_CHARS {
            return Err(ProviderError::Rejected {
                code: "BODY_TOO_LONG".to_owned(),
                message: format!("AWS accepts at most {MAX_BODY_CHARS} characters per message"),
            });
        }
        let origination_identity = send
            .config
            .origination_identity
            .as_deref()
            .or_else(|| (!req.sender_id.trim().is_empty()).then_some(req.sender_id.as_str()));

        // Serialised once: these exact bytes are both what is signed and
        // what is sent.
        let body = serde_json::to_vec(&SendTextMessageRequest {
            destination_phone_number: &req.to,
            origination_identity,
            message_body: &req.body,
            message_type: send.config.message_type.wire(),
            configuration_set_name: &send.config.configuration_set,
            context: BTreeMap::from([(CONTEXT_MESSAGE_ID, req.reference.as_str())]),
        })
        .expect("a struct of strings always serialises");

        let signature = sigv4::sign(
            &sigv4::Credentials {
                access_key_id: &send.config.access_key_id,
                secret_access_key: &send.config.secret_access_key,
            },
            &sigv4::Request {
                method: "POST",
                host: &send.host,
                path: send.endpoint.path(),
                headers: &[("content-type", JSON_1_0), ("x-amz-target", TARGET)],
                body: &body,
                region: &send.config.region,
                service: SERVICE,
            },
            Utc::now(),
        );

        let response = self
            .client
            .post(send.endpoint.clone())
            .header(CONTENT_TYPE, JSON_1_0)
            .header("x-amz-target", TARGET)
            .header("x-amz-date", &signature.amz_date)
            .header(AUTHORIZATION, &signature.authorization)
            .body(body)
            .send()
            .await
            .map_err(|error| classify_transport_error(error, PROVIDER_NOUN))?;

        let status = response.status();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let text = response.text().await;

        if status.is_success() {
            // AWS has accepted the message from here on, so anything that
            // stops us learning its id is `Indeterminate`, never a retry.
            let text = text.map_err(|error| ProviderError::Indeterminate {
                message: format!(
                    "AWS accepted the message but the response could not be read: {error}"
                ),
                source: Some(Box::new(error)),
            })?;
            return match serde_json::from_str::<SendTextMessageResponse>(&text) {
                Ok(SendTextMessageResponse {
                    message_id: Some(id),
                }) if !id.trim().is_empty() => Ok(SubmitAck {
                    provider_ref: id,
                    provider_ref_alt: None,
                }),
                _ => Err(ProviderError::Indeterminate {
                    message: format!("AWS answered {status} without a MessageId: {text}"),
                    source: None,
                }),
            };
        }
        Err(classify_error(
            status,
            error_type.as_deref(),
            &text.unwrap_or_default(),
        ))
    }

    async fn authenticate_callback(
        &self,
        raw: &RawCallback,
    ) -> Result<CallbackDisposition, ProviderError> {
        let Some(verifier) = &self.verifier else {
            return Ok(CallbackDisposition::Refused {
                reason: "this process has no AWS SMS events topic configured".to_owned(),
            });
        };
        let envelope = match sns::Envelope::parse(&raw.body) {
            Ok(envelope) => envelope,
            Err(reason) => return Ok(CallbackDisposition::Refused { reason }),
        };
        if let sns::Verdict::Refused(reason) = verifier.verify(&envelope).await? {
            return Ok(CallbackDisposition::Refused { reason });
        }
        match envelope.kind.as_str() {
            "Notification" => Ok(CallbackDisposition::Ingest),
            "SubscriptionConfirmation" => {
                verifier.confirm_subscription(&envelope).await?;
                Ok(CallbackDisposition::Acknowledged {
                    detail: format!("confirmed the SNS subscription to {}", envelope.topic_arn),
                })
            }
            "UnsubscribeConfirmation" => {
                tracing::warn!(
                    topic_arn = envelope.topic_arn,
                    "the SNS subscription delivering AWS SMS receipts was removed; no further \
                     receipts will arrive until it is re-created"
                );
                Ok(CallbackDisposition::Acknowledged {
                    detail: "noted an SNS unsubscribe confirmation".to_owned(),
                })
            }
            other => Ok(CallbackDisposition::Refused {
                reason: format!("unexpected SNS message Type {other:?}"),
            }),
        }
    }

    fn parse_dlr(&self, raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
        let envelope =
            sns::Envelope::parse(&raw.body).map_err(|message| ProviderError::Rejected {
                code: "MALFORMED_DLR".to_owned(),
                message,
            })?;
        if envelope.kind != "Notification" {
            return Ok(Vec::new());
        }
        events::parse(&envelope.message)
    }

    async fn health(&self) -> Health {
        // There is no free, side-effect-free call that proves the account
        // can send; a real failure surfaces through `submit` and the
        // circuit breaker instead.
        Health::ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageType, classify_error, is_region};
    use reqwest::StatusCode;
    use sms_provider::ProviderError;

    #[test]
    fn throttling_is_transient() {
        let error = classify_error(
            StatusCode::BAD_REQUEST,
            Some("ThrottlingException"),
            r#"{"message":"Rate exceeded"}"#,
        );
        assert!(matches!(error, ProviderError::Transient { .. }));
    }

    #[test]
    fn the_error_type_is_read_from_the_body_when_the_header_is_absent() {
        let error = classify_error(
            StatusCode::BAD_REQUEST,
            None,
            r#"{"__type":"com.amazonaws.pinpointsmsvoicev2#ThrottlingException","Message":"slow down"}"#,
        );
        assert!(
            matches!(error, ProviderError::Transient { message, .. } if message == "slow down")
        );
    }

    #[test]
    fn the_header_suffix_is_stripped() {
        let error = classify_error(
            StatusCode::BAD_REQUEST,
            Some(
                "UnrecognizedClientException:http://internal.amazon.com/coral/com.amazon.coral.service/",
            ),
            "{}",
        );
        assert!(matches!(error, ProviderError::Unavailable { .. }));
    }

    /// Spend limit, bad credentials, a missing configuration set: nothing
    /// was sent and nothing this message could change would help, so fail
    /// over and count toward the breaker.
    #[test]
    fn account_level_failures_are_unavailable() {
        for kind in [
            "ServiceQuotaExceededException",
            "AccessDeniedException",
            "ResourceNotFoundException",
            "InvalidSignatureException",
        ] {
            let error = classify_error(StatusCode::BAD_REQUEST, Some(kind), r#"{"Reason":"X"}"#);
            assert!(matches!(error, ProviderError::Unavailable { .. }), "{kind}");
        }
    }

    #[test]
    fn a_route_specific_validation_failure_tries_the_next_route() {
        let error = classify_error(
            StatusCode::BAD_REQUEST,
            Some("ValidationException"),
            r#"{"Message":"no","Reason":"DESTINATION_COUNTRY_BLOCKED"}"#,
        );
        assert!(
            matches!(error, ProviderError::Permanent { code, .. } if code == "DESTINATION_COUNTRY_BLOCKED")
        );
    }

    #[test]
    fn a_request_validation_failure_is_rejected() {
        let error = classify_error(
            StatusCode::BAD_REQUEST,
            Some("ValidationException"),
            r#"{"Message":"bad number","Reason":"FIELD_VALIDATION_FAILED"}"#,
        );
        assert!(
            matches!(error, ProviderError::Rejected { code, .. } if code == "FIELD_VALIDATION_FAILED")
        );
    }

    #[test]
    fn an_opted_out_destination_is_rejected() {
        let error = classify_error(
            StatusCode::BAD_REQUEST,
            Some("ConflictException"),
            r#"{"Reason":"DESTINATION_PHONE_NUMBER_OPTED_OUT"}"#,
        );
        assert!(matches!(error, ProviderError::Rejected { .. }));
    }

    #[test]
    fn an_internal_error_is_unavailable() {
        let error = classify_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            Some("InternalServerException"),
            "{}",
        );
        assert!(matches!(error, ProviderError::Unavailable { .. }));
    }

    #[test]
    fn region_shape() {
        for ok in ["eu-west-1", "af-south-1", "us-gov-west-1"] {
            assert!(is_region(ok), "{ok}");
        }
        for bad in [
            "",
            "EU-WEST-1",
            "euwest1",
            "-eu-west-1",
            "eu-west-1-",
            "eu_west_1",
            "eu-west-1.evil",
        ] {
            assert!(!is_region(bad), "{bad}");
        }
    }

    #[test]
    fn message_type_parses_case_insensitively() {
        assert_eq!("TRANSACTIONAL".parse(), Ok(MessageType::Transactional));
        assert_eq!("promotional".parse(), Ok(MessageType::Promotional));
        assert!("otp".parse::<MessageType>().is_err());
    }
}
