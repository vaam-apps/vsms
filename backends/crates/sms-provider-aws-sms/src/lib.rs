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

/// `SendTextMessage`'s `TimeToLive` bounds in seconds (botocore model:
/// `min` 5, `max` 259200).
const MIN_TIME_TO_LIVE_SECS: u64 = 5;
const MAX_TIME_TO_LIVE_SECS: u64 = 259_200;

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
    /// How long AWS keeps trying to deliver one message, sent as
    /// `TimeToLive` (5 seconds to 72 hours). `None` leaves AWS's own
    /// default of 72 hours, so an OTP vsms already expired can still
    /// arrive days later; see `lib.md`.
    pub time_to_live: Option<Duration>,
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
            .field("time_to_live", &self.time_to_live)
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
    /// `TimeToLive` is outside what AWS accepts.
    #[error("time to live {0:?} must be between 5 seconds and 72 hours")]
    TimeToLive(Duration),
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
    pub fn sender(mut config: AwsSmsConfig) -> Result<Self, ConfigError> {
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
        // An empty environment variable reaches here as `Some("")`; it
        // means "unset", not an identity or endpoint named "".
        config.origination_identity.take_if(|v| v.trim().is_empty());
        config.endpoint_url.take_if(|v| v.trim().is_empty());
        if let Some(ttl) = config.time_to_live
            && !(MIN_TIME_TO_LIVE_SECS..=MAX_TIME_TO_LIVE_SECS).contains(&ttl.as_secs())
        {
            return Err(ConfigError::TimeToLive(ttl));
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
    /// supplied by the caller — the seam
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
    #[serde(skip_serializing_if = "Option::is_none")]
    time_to_live: Option<u64>,
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
    #[serde(rename = "ResourceType", default)]
    resource_type: Option<String>,
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
/// message: credentials, account standing, or a quota or spend limit. Each
/// fails over and counts toward the provider circuit breaker, and none of
/// them sent anything. `AccessDeniedException` and
/// `ResourceNotFoundException` are here too, except where they can only be
/// about the message's own sender identity; see [`is_sender_specific`].
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

/// `ConflictException` reasons where retrying anywhere fails the same way:
/// the destination itself refuses this account.
const CONFLICT_REJECTED: &[&str] = &[
    "DESTINATION_PHONE_NUMBER_BLOCKED_BY_PROTECT_NUMBER_OVERRIDE",
    "DESTINATION_PHONE_NUMBER_OPTED_OUT",
];

/// `ConflictException` reasons that mean this account's setup cannot send
/// right now, whatever the message: fail over and count toward the breaker.
const CONFLICT_UNAVAILABLE: &[&str] = &["MESSAGE_TYPE_MISMATCH", "RESOURCE_NOT_ACTIVE"];

/// Whether a `SendTextMessage` error is about the message's own sender
/// identity rather than the account. Only possible when the identity came
/// from the request (no configured override): `AccessDeniedException`
/// without a `Reason`, or `ResourceNotFoundException` for anything but the
/// configuration set, then name that identity, and counting them toward the
/// breaker would let one app's unregistered sender ID take every other
/// app's traffic off AWS.
fn is_sender_specific(kind: &str, parsed: &AwsErrorBody) -> bool {
    match kind {
        "AccessDeniedException" => parsed.reason.is_none(),
        "ResourceNotFoundException" => parsed.resource_type.as_deref() != Some("configuration-set"),
        _ => false,
    }
}

/// Classify a non-2xx `SendTextMessage` response. `error_type` is the
/// `x-amzn-ErrorType` header when present; the body's `__type` otherwise.
/// `sender_from_request` is whether `OriginationIdentity` came from the
/// message's own sender ID rather than the configured override.
fn classify_error(
    status: StatusCode,
    error_type: Option<&str>,
    body: &str,
    sender_from_request: bool,
) -> ProviderError {
    let parsed: AwsErrorBody = serde_json::from_str(body).unwrap_or_default();
    // Either source may be `com.amazonaws.…#ThrottlingException`, and the
    // header may carry a `:http://…` suffix; the bare name is what matters.
    let kind = error_type
        .map(str::to_owned)
        .or_else(|| parsed.kind.clone())
        .map(|raw| {
            let name = raw.rsplit('#').next().unwrap_or(&raw);
            name.split(':').next().unwrap_or(name).to_owned()
        })
        .unwrap_or_default();
    let message = parsed.message.clone().unwrap_or_else(|| body.to_owned());

    if kind == "ThrottlingException" {
        return ProviderError::Transient {
            retry_after: THROTTLE_RETRY_AFTER,
            message,
        };
    }
    if sender_from_request && is_sender_specific(&kind, &parsed) {
        return ProviderError::Permanent {
            code: kind,
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
        let reason = parsed.reason.unwrap_or(kind);
        if CONFLICT_REJECTED.contains(&reason.as_str()) {
            return ProviderError::Rejected {
                code: reason,
                message,
            };
        }
        if CONFLICT_UNAVAILABLE.contains(&reason.as_str()) {
            return ProviderError::Unavailable {
                message: format!("AWS cannot send from this account setup: {reason}: {message}"),
                source: None,
            };
        }
        // Anything else is about this message on this route (an unverified
        // sandbox destination, no origination identity for the country, a
        // blocked country: another route may carry it) or an
        // administrative operation `SendTextMessage` cannot raise. Failing
        // over is the safe default; see `lib.md`.
        return ProviderError::Permanent {
            code: reason,
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
        let sender_from_request =
            send.config.origination_identity.is_none() && origination_identity.is_some();

        // Serialised once: these exact bytes are both what is signed and
        // what is sent.
        let body = serde_json::to_vec(&SendTextMessageRequest {
            destination_phone_number: &req.to,
            origination_identity,
            message_body: &req.body,
            message_type: send.config.message_type.wire(),
            configuration_set_name: &send.config.configuration_set,
            time_to_live: send.config.time_to_live.map(|ttl| ttl.as_secs()),
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
            sender_from_request,
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
        match verifier.verify(&envelope).await? {
            sns::Verdict::Authentic => {}
            sns::Verdict::Refused(reason) => return Ok(CallbackDisposition::Refused { reason }),
            sns::Verdict::UnsupportedSignatureVersion(reason) => {
                // The topic is ours, so this is a misconfiguration, and SNS
                // treats the 403 as permanent: every receipt is lost.
                tracing::error!(
                    topic_arn = %envelope.topic_arn,
                    "dropping an SNS delivery for the AWS SMS events topic: {reason}"
                );
                return Ok(CallbackDisposition::Refused { reason });
            }
        }
        match envelope.kind.as_str() {
            "Notification" => Ok(CallbackDisposition::Ingest),
            "SubscriptionConfirmation" => {
                if let Err(reason) = verifier.check_subscribe_url(&envelope) {
                    return Ok(CallbackDisposition::Refused { reason });
                }
                // Deliberately not visited: confirming by GET leaves
                // `AuthenticateOnUnsubscribe` false, so anyone holding a
                // notification's `UnsubscribeURL` could remove the
                // subscription. See `sns.md`.
                let token = envelope.token.as_deref().unwrap_or_default();
                tracing::warn!(
                    topic_arn = %envelope.topic_arn,
                    token,
                    "SNS subscription awaiting confirmation; confirm it with: aws sns \
                     confirm-subscription --topic-arn {} --token {token} \
                     --authenticate-on-unsubscribe true",
                    envelope.topic_arn
                );
                Ok(CallbackDisposition::Acknowledged {
                    detail: format!(
                        "SNS subscription to {} is not confirmed; see the gateway log",
                        envelope.topic_arn
                    ),
                })
            }
            "UnsubscribeConfirmation" => {
                tracing::error!(
                    topic_arn = %envelope.topic_arn,
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
    use super::{AwsErrorBody, MessageType, classify_error, is_region, is_sender_specific};
    use reqwest::StatusCode;
    use sms_provider::ProviderError;

    fn classify(kind: &str, body: &str, sender_from_request: bool) -> ProviderError {
        classify_error(
            StatusCode::BAD_REQUEST,
            Some(kind),
            body,
            sender_from_request,
        )
    }

    #[test]
    fn throttling_is_transient() {
        let error = classify_error(
            StatusCode::BAD_REQUEST,
            Some("ThrottlingException"),
            r#"{"message":"Rate exceeded"}"#,
            false,
        );
        assert!(matches!(error, ProviderError::Transient { .. }));
    }

    #[test]
    fn the_error_type_is_read_from_the_body_when_the_header_is_absent() {
        let error = classify_error(
            StatusCode::BAD_REQUEST,
            None,
            r#"{"__type":"com.amazonaws.pinpointsmsvoicev2#ThrottlingException","Message":"slow down"}"#,
            false,
        );
        assert!(
            matches!(error, ProviderError::Transient { message, .. } if message == "slow down")
        );
    }

    #[test]
    fn the_header_suffix_is_stripped() {
        let error = classify(
            "UnrecognizedClientException:http://internal.amazon.com/coral/com.amazon.coral.service/",
            "{}",
            false,
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
            let error = classify(kind, r#"{"Reason":"X"}"#, false);
            assert!(matches!(error, ProviderError::Unavailable { .. }), "{kind}");
        }
    }

    /// With a configured identity there is nothing per-message to blame, so
    /// a missing resource or a denial is the account's.
    #[test]
    fn without_a_request_sender_access_and_lookup_failures_are_account_level() {
        for kind in ["AccessDeniedException", "ResourceNotFoundException"] {
            let error = classify(kind, r#"{"Message":"nope"}"#, false);
            assert!(matches!(error, ProviderError::Unavailable { .. }), "{kind}");
        }
    }

    /// One app's unregistered sender ID must not open the breaker for
    /// everyone else's traffic.
    #[test]
    fn a_request_sender_that_does_not_exist_or_is_denied_fails_the_route_only() {
        for (kind, body) in [
            ("AccessDeniedException", r#"{"Message":"not allowed"}"#),
            (
                "ResourceNotFoundException",
                r#"{"Message":"no such sender","ResourceType":"sender-id","ResourceId":"VYMALO"}"#,
            ),
            (
                "ResourceNotFoundException",
                r#"{"Message":"no such thing"}"#,
            ),
        ] {
            let error = classify(kind, body, true);
            assert!(
                matches!(&error, ProviderError::Permanent { code, .. } if code == kind),
                "{kind}: {error:?}"
            );
        }
    }

    #[test]
    fn account_standing_and_a_missing_configuration_set_stay_account_level_with_a_request_sender() {
        for (kind, body) in [
            (
                "AccessDeniedException",
                r#"{"Reason":"ACCOUNT_DISABLED","Message":"x"}"#,
            ),
            (
                "AccessDeniedException",
                r#"{"Reason":"INSUFFICIENT_ACCOUNT_REPUTATION","Message":"x"}"#,
            ),
            (
                "ResourceNotFoundException",
                r#"{"ResourceType":"configuration-set","ResourceId":"vsms-events","Message":"x"}"#,
            ),
        ] {
            let error = classify(kind, body, true);
            assert!(
                matches!(error, ProviderError::Unavailable { .. }),
                "{kind} {body}"
            );
        }
    }

    #[test]
    fn only_those_two_error_types_can_be_sender_specific() {
        let parsed = AwsErrorBody::default();
        assert!(is_sender_specific("AccessDeniedException", &parsed));
        assert!(is_sender_specific("ResourceNotFoundException", &parsed));
        assert!(!is_sender_specific(
            "ServiceQuotaExceededException",
            &parsed
        ));
        assert!(!is_sender_specific("InvalidSignatureException", &parsed));
    }

    #[test]
    fn a_route_specific_validation_failure_tries_the_next_route() {
        let error = classify(
            "ValidationException",
            r#"{"Message":"no","Reason":"DESTINATION_COUNTRY_BLOCKED"}"#,
            false,
        );
        assert!(
            matches!(error, ProviderError::Permanent { code, .. } if code == "DESTINATION_COUNTRY_BLOCKED")
        );
    }

    #[test]
    fn a_request_validation_failure_is_rejected() {
        let error = classify(
            "ValidationException",
            r#"{"Message":"bad number","Reason":"FIELD_VALIDATION_FAILED"}"#,
            false,
        );
        assert!(
            matches!(error, ProviderError::Rejected { code, .. } if code == "FIELD_VALIDATION_FAILED")
        );
    }

    #[test]
    fn an_opted_out_destination_is_rejected() {
        let error = classify(
            "ConflictException",
            r#"{"Reason":"DESTINATION_PHONE_NUMBER_OPTED_OUT"}"#,
            false,
        );
        assert!(matches!(error, ProviderError::Rejected { .. }));
    }

    /// What the classifier must do with each of `ConflictExceptionReason`.
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    enum Expected {
        Rejected,
        Permanent,
        Unavailable,
    }

    /// `ConflictExceptionReason` from botocore 1.43.116's
    /// `pinpoint-sms-voice-v2` model, every member, each with the outcome
    /// it must have. Most are administrative operations `SendTextMessage`
    /// cannot raise and take the default, `Permanent`.
    const CONFLICT_REASONS: &[(&str, Expected)] = &[
        (
            "CREATE_REGISTRATION_VERSION_NOT_ALLOWED",
            Expected::Permanent,
        ),
        ("DELETION_PROTECTION_ENABLED", Expected::Permanent),
        ("DESTINATION_PHONE_NUMBER_NOT_VERIFIED", Expected::Permanent),
        ("DESTINATION_PHONE_NUMBER_OPTED_OUT", Expected::Rejected),
        ("DISASSOCIATE_REGISTRATION_NOT_ALLOWED", Expected::Permanent),
        (
            "DISCARD_REGISTRATION_VERSION_NOT_ALLOWED",
            Expected::Permanent,
        ),
        (
            "EDIT_REGISTRATION_FIELD_VALUES_NOT_ALLOWED",
            Expected::Permanent,
        ),
        ("EVENT_DESTINATION_MISMATCH", Expected::Permanent),
        ("KEYWORD_MISMATCH", Expected::Permanent),
        ("LAST_PHONE_NUMBER", Expected::Permanent),
        ("NUMBER_CAPABILITIES_MISMATCH", Expected::Permanent),
        ("MESSAGE_TYPE_MISMATCH", Expected::Unavailable),
        ("NO_ORIGINATION_IDENTITIES_FOUND", Expected::Permanent),
        ("OPT_OUT_LIST_MISMATCH", Expected::Permanent),
        ("PHONE_NUMBER_ASSOCIATED_TO_POOL", Expected::Permanent),
        (
            "PHONE_NUMBER_ASSOCIATED_TO_REGISTRATION",
            Expected::Permanent,
        ),
        ("PHONE_NUMBER_NOT_ASSOCIATED_TO_POOL", Expected::Permanent),
        (
            "PHONE_NUMBER_NOT_IN_REGISTRATION_REGION",
            Expected::Permanent,
        ),
        ("REGISTRATION_ALREADY_SUBMITTED", Expected::Permanent),
        ("REGISTRATION_NOT_COMPLETE", Expected::Permanent),
        ("RESOURCE_ALREADY_ASSOCIATED", Expected::Permanent),
        ("SENDER_ID_ASSOCIATED_TO_POOL", Expected::Permanent),
        ("RCS_AGENT_ASSOCIATED_TO_POOL", Expected::Permanent),
        (
            "POOL_ASSOCIATED_TO_NOTIFY_CONFIGURATION",
            Expected::Permanent,
        ),
        ("RESOURCE_ALREADY_EXISTS", Expected::Permanent),
        ("RESOURCE_DELETION_NOT_ALLOWED", Expected::Permanent),
        ("RESOURCE_MODIFICATION_NOT_ALLOWED", Expected::Permanent),
        ("RESOURCE_NOT_ACTIVE", Expected::Unavailable),
        ("RESOURCE_NOT_EMPTY", Expected::Permanent),
        ("SELF_MANAGED_OPT_OUTS_MISMATCH", Expected::Permanent),
        (
            "SUBMIT_REGISTRATION_VERSION_NOT_ALLOWED",
            Expected::Permanent,
        ),
        ("TWO_WAY_CONFIG_MISMATCH", Expected::Permanent),
        ("VERIFICATION_CODE_EXPIRED", Expected::Permanent),
        ("VERIFICATION_ALREADY_COMPLETE", Expected::Permanent),
        (
            "PROTECT_CONFIGURATION_IS_ACCOUNT_DEFAULT",
            Expected::Permanent,
        ),
        (
            "PROTECT_CONFIGURATION_ASSOCIATED_WITH_CONFIGURATION_SET",
            Expected::Permanent,
        ),
        (
            "PROTECT_CONFIGURATION_NOT_ASSOCIATED_WITH_CONFIGURATION_SET",
            Expected::Permanent,
        ),
        (
            "DESTINATION_COUNTRY_BLOCKED_BY_PROTECT_CONFIGURATION",
            Expected::Permanent,
        ),
        (
            "DESTINATION_PHONE_NUMBER_BLOCKED_BY_PROTECT_NUMBER_OVERRIDE",
            Expected::Rejected,
        ),
        (
            "RCS_AGENT_ALREADY_ASSOCIATED_TO_REGISTRATION_TYPE",
            Expected::Permanent,
        ),
        ("NOTIFY_CONFIGURATION_NOT_ACTIVE", Expected::Permanent),
    ];

    #[test]
    fn every_conflict_reason_has_a_deliberate_outcome() {
        assert_eq!(CONFLICT_REASONS.len(), 41, "botocore 1.43.116 lists 41");
        for (reason, expected) in CONFLICT_REASONS {
            let error = classify(
                "ConflictException",
                &format!(r#"{{"Message":"m","Reason":"{reason}"}}"#),
                false,
            );
            let actual = match error {
                ProviderError::Rejected { .. } => Expected::Rejected,
                ProviderError::Permanent { .. } => Expected::Permanent,
                ProviderError::Unavailable { .. } => Expected::Unavailable,
                other => panic!("{reason}: unexpected {other:?}"),
            };
            assert_eq!(actual, *expected, "{reason}");
        }
    }

    #[test]
    fn a_conflict_with_no_or_an_unknown_reason_fails_the_route_not_the_message() {
        for body in [
            r#"{"Message":"m"}"#,
            r#"{"Reason":"A_REASON_AWS_ADDED_LATER"}"#,
        ] {
            let error = classify("ConflictException", body, false);
            assert!(matches!(error, ProviderError::Permanent { .. }), "{body}");
        }
    }

    #[test]
    fn an_internal_error_is_unavailable() {
        let error = classify_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            Some("InternalServerException"),
            "{}",
            false,
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
