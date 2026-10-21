#![doc = include_str!("lib.md")]

mod ledger;
mod policy;
mod redirect;
mod signing;
mod sigv4;

pub use ledger::{Ledger, NotificationRecord, SubmitRecord};
pub use policy::{EventStep, FaultPolicy, SubmitDecision, SubmitResponse};
pub use redirect::RedirectSnsHttp;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::json;
use tokio::task::JoinHandle;

use signing::SigningIdentity;

/// Region the fake's defaults use.
pub const DEFAULT_REGION: &str = "eu-west-1";
/// Access key id the fake's defaults recognise. Not an AWS key.
pub const DEFAULT_ACCESS_KEY_ID: &str = "AKIAFAKEAWSSMSDEMO00";
/// Secret the fake's defaults recognise. Not an AWS secret.
pub const DEFAULT_SECRET_ACCESS_KEY: &str = "fake-aws-secret-access-key-not-a-real-one";
/// Configuration set the fake's defaults expect.
pub const DEFAULT_CONFIGURATION_SET: &str = "vsms-fake";
/// Events topic the fake's defaults publish to. Account `000000000000`
/// does not exist.
pub const DEFAULT_TOPIC_ARN: &str = "arn:aws:sns:eu-west-1:000000000000:vsms-sms-events";

/// `SendTextMessage`'s `X-Amz-Target`.
const SEND_TEXT_MESSAGE: &str = "PinpointSMSVoiceV2.SendTextMessage";
/// Signature Version 4 signing name of the service.
const SERVICE: &str = "sms-voice";
const JSON_1_0: &str = "application/x-amz-json-1.0";

/// How the fake retries a notification the endpoint did not acknowledge.
/// SNS's own default HTTP/S policy is three retries twenty seconds apart.
#[derive(Debug, Clone, Copy)]
pub struct Redelivery {
    /// Total POST attempts, including the first.
    pub attempts: u32,
    /// Pause between attempts.
    pub interval: Duration,
}

impl Default for Redelivery {
    fn default() -> Self {
        Self {
            attempts: 4,
            interval: Duration::from_secs(20),
        }
    }
}

/// The account the fake pretends to be.
#[derive(Debug, Clone)]
pub struct FakeAwsConfig {
    /// AWS region, as both the Signature Version 4 scope and the topic's.
    pub region: String,
    /// The one access key id accepted.
    pub access_key_id: String,
    /// Its secret.
    pub secret_access_key: String,
    /// The one configuration set that exists.
    pub configuration_set: String,
    /// The topic notifications claim to come from.
    pub topic_arn: String,
    /// Full URL notifications are posted to, e.g.
    /// `http://sms-gateway:8080/dlr/aws_sms`.
    pub dlr_endpoint: String,
    /// See [`Redelivery`].
    pub redelivery: Redelivery,
}

impl FakeAwsConfig {
    /// The `DEFAULT_*` account, publishing to `dlr_endpoint`.
    #[must_use]
    pub fn with_defaults(dlr_endpoint: impl Into<String>) -> Self {
        Self {
            region: DEFAULT_REGION.to_owned(),
            access_key_id: DEFAULT_ACCESS_KEY_ID.to_owned(),
            secret_access_key: DEFAULT_SECRET_ACCESS_KEY.to_owned(),
            configuration_set: DEFAULT_CONFIGURATION_SET.to_owned(),
            topic_arn: DEFAULT_TOPIC_ARN.to_owned(),
            dlr_endpoint: dlr_endpoint.into(),
            redelivery: Redelivery::default(),
        }
    }
}

struct Inner {
    config: FakeAwsConfig,
    policy: FaultPolicy,
    ledger: Arc<Ledger>,
    identity: SigningIdentity,
    client: reqwest::Client,
}

/// A running fake. Dropping it stops the server; notifications already
/// scheduled still go out.
pub struct FakeAws {
    inner: Arc<Inner>,
    base_url: String,
    server: JoinHandle<()>,
}

impl Drop for FakeAws {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl FakeAws {
    /// Start on an ephemeral loopback port.
    ///
    /// # Panics
    ///
    /// When no loopback port can be bound.
    pub async fn start(config: FakeAwsConfig, policy: FaultPolicy) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding an ephemeral loopback port for sms-fake-aws");
        Self::start_on(listener, config, policy)
    }

    /// Start on `listener`, for a long-lived process with a fixed address.
    ///
    /// # Panics
    ///
    /// When `listener`'s address cannot be read back.
    #[must_use]
    pub fn start_on(
        listener: tokio::net::TcpListener,
        config: FakeAwsConfig,
        policy: FaultPolicy,
    ) -> Self {
        let addr = listener
            .local_addr()
            .expect("reading the fake's bound address");
        let inner = Arc::new(Inner {
            config,
            policy,
            ledger: Arc::new(Ledger::default()),
            identity: SigningIdentity::generate(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest client builder with a timeout never fails"),
        });
        let app = Router::new()
            .route("/", post(send_text_message))
            .route("/{file}", get(signing_cert))
            .with_state(inner.clone());
        let server = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                tracing::error!(%error, "sms-fake-aws server stopped");
            }
        });
        Self {
            inner,
            base_url: format!("http://{addr}/"),
            server,
        }
    }

    /// Where to point `AwsSmsConfig::endpoint_url` (the `SendTextMessage`
    /// endpoint) and [`RedirectSnsHttp`] (the signing certificate), with a
    /// trailing slash.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// What the fake received and sent.
    #[must_use]
    pub fn ledger(&self) -> &Ledger {
        &self.inner.ledger
    }

    /// The topic notifications claim to come from.
    #[must_use]
    pub fn topic_arn(&self) -> &str {
        &self.inner.config.topic_arn
    }

    /// The `SigningCertURL` every notification carries: on AWS's real SNS
    /// host for the topic's region, so the receiver's URL checks apply
    /// unchanged. Fetching it reaches AWS, not the fake, unless the
    /// receiver was built with [`RedirectSnsHttp`].
    #[must_use]
    pub fn signing_cert_url(&self) -> String {
        format!(
            "https://sns.{}.amazonaws.com/{}",
            self.inner.config.region,
            self.inner.identity.cert_file()
        )
    }

    /// A signed `Notification` envelope carrying `message`, exactly as one
    /// would be posted. Lets a test tamper with a genuine one.
    #[must_use]
    pub fn signed_notification(&self, message: &str) -> String {
        self.inner.notification(message).to_string()
    }

    /// Publish a signed `SubscriptionConfirmation` to the endpoint, as SNS
    /// does once when a subscription is created, and return the status the
    /// endpoint answered.
    ///
    /// # Errors
    ///
    /// When the endpoint could not be reached.
    pub async fn send_subscription_confirmation(&self) -> Result<u16, String> {
        let token = random_hex(32);
        let envelope = self.inner.subscription_confirmation(&token);
        self.inner
            .post_envelope("SubscriptionConfirmation", &envelope)
            .await
    }
}

impl Inner {
    fn cert_url(&self) -> String {
        format!(
            "https://sns.{}.amazonaws.com/{}",
            self.config.region,
            self.identity.cert_file()
        )
    }

    fn notification(&self, message: &str) -> serde_json::Value {
        let (message_id, timestamp) = fresh_id_and_timestamp();
        let topic = &self.config.topic_arn;
        let signature = self.identity.sign(
            signing::string_to_sign(&[
                ("Message", message),
                ("MessageId", &message_id),
                ("Timestamp", &timestamp),
                ("TopicArn", topic),
                ("Type", "Notification"),
            ])
            .as_bytes(),
        );
        json!({
            "Type": "Notification",
            "MessageId": message_id,
            "TopicArn": topic,
            "Message": message,
            "Timestamp": timestamp,
            "SignatureVersion": "2",
            "Signature": signature,
            "SigningCertURL": self.cert_url(),
            "UnsubscribeURL": format!(
                "https://sns.{}.amazonaws.com/?Action=Unsubscribe&SubscriptionArn={topic}:{}",
                self.config.region,
                uuid_like()
            ),
        })
    }

    fn subscription_confirmation(&self, token: &str) -> serde_json::Value {
        let (message_id, timestamp) = fresh_id_and_timestamp();
        let topic = &self.config.topic_arn;
        let subscribe_url = format!(
            "https://sns.{}.amazonaws.com/?Action=ConfirmSubscription&TopicArn={topic}&Token={token}",
            self.config.region
        );
        let message = format!(
            "You have chosen to subscribe to the topic {topic}.\nTo confirm the subscription, visit the SubscribeURL included in this message."
        );
        let signature = self.identity.sign(
            signing::string_to_sign(&[
                ("Message", &message),
                ("MessageId", &message_id),
                ("SubscribeURL", &subscribe_url),
                ("Timestamp", &timestamp),
                ("Token", token),
                ("TopicArn", topic),
                ("Type", "SubscriptionConfirmation"),
            ])
            .as_bytes(),
        );
        json!({
            "Type": "SubscriptionConfirmation",
            "MessageId": message_id,
            "Token": token,
            "TopicArn": topic,
            "Message": message,
            "SubscribeURL": subscribe_url,
            "Timestamp": timestamp,
            "SignatureVersion": "2",
            "Signature": signature,
            "SigningCertURL": self.cert_url(),
        })
    }

    /// One POST, with the headers SNS sends. The status, or why there was
    /// none.
    async fn post_envelope(&self, kind: &str, envelope: &serde_json::Value) -> Result<u16, String> {
        let response = self
            .client
            .post(&self.config.dlr_endpoint)
            .header(header::CONTENT_TYPE, "text/plain; charset=UTF-8")
            .header("x-amz-sns-message-type", kind)
            .header(
                "x-amz-sns-message-id",
                envelope["MessageId"].as_str().unwrap_or_default(),
            )
            .header("x-amz-sns-topic-arn", &self.config.topic_arn)
            .header(
                header::USER_AGENT,
                "Amazon Simple Notification Service Agent",
            )
            .body(envelope.to_string())
            .send()
            .await
            .map_err(|error| format!("POST {} failed: {error}", self.config.dlr_endpoint))?;
        Ok(response.status().as_u16())
    }

    /// Publish one event, retrying per [`Redelivery`] until a 2xx.
    async fn publish(self: Arc<Self>, event: serde_json::Value, step: EventStep) {
        let message_id = event["messageId"].as_str().unwrap_or_default().to_owned();
        let envelope = self.notification(&event.to_string());
        let mut attempts = 0;
        let mut last_status = None;
        let mut acknowledged = false;
        while attempts < self.config.redelivery.attempts.max(1) {
            if attempts > 0 {
                tokio::time::sleep(self.config.redelivery.interval).await;
            }
            attempts += 1;
            match self.post_envelope("Notification", &envelope).await {
                Ok(status) => {
                    last_status = Some(status);
                    if (200..300).contains(&status) {
                        acknowledged = true;
                        break;
                    }
                    tracing::warn!(
                        message_id,
                        status,
                        attempts,
                        "notification not acknowledged"
                    );
                }
                Err(error) => {
                    tracing::warn!(message_id, %error, attempts, "notification not delivered");
                }
            }
        }
        tracing::info!(
            message_id,
            event_type = step.event_type,
            attempts,
            acknowledged,
            "published a delivery event"
        );
        self.ledger.record_notification(NotificationRecord {
            message_id,
            event_type: step.event_type,
            is_final: step.is_final,
            attempts,
            last_status,
            acknowledged,
        });
    }
}

/// The members of `SendTextMessage` the fake reads (botocore's model).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SendTextMessageRequest {
    destination_phone_number: String,
    #[serde(default)]
    origination_identity: Option<String>,
    #[serde(default)]
    message_body: Option<String>,
    #[serde(default)]
    message_type: Option<String>,
    #[serde(default)]
    configuration_set_name: Option<String>,
    #[serde(default)]
    time_to_live: Option<u64>,
    #[serde(default)]
    context: BTreeMap<String, String>,
}

/// An AWS JSON 1.0 error answer.
struct AwsError {
    status: u16,
    error_type: String,
    reason: Option<String>,
    message: String,
    /// `ResourceNotFoundException`'s `ResourceType`.
    resource_type: Option<&'static str>,
}

impl AwsError {
    fn new(
        status: u16,
        error_type: &str,
        reason: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            error_type: error_type.to_owned(),
            reason: reason.map(str::to_owned),
            message: message.into(),
            resource_type: None,
        }
    }

    fn into_response(self) -> Response {
        let mut body = json!({
            "__type": format!("com.amazonaws.pinpointsmsvoicev2#{}", self.error_type),
            "Message": self.message,
        });
        if let Some(reason) = &self.reason {
            body["Reason"] = json!(reason);
        }
        if let Some(resource_type) = self.resource_type {
            body["ResourceType"] = json!(resource_type);
        }
        let mut response = (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_REQUEST),
            [(header::CONTENT_TYPE, JSON_1_0)],
            body.to_string(),
        )
            .into_response();
        if let Ok(value) = HeaderValue::from_str(&self.error_type) {
            response.headers_mut().insert("x-amzn-errortype", value);
        }
        response
    }
}

async fn signing_cert(State(inner): State<Arc<Inner>>, Path(file): Path<String>) -> Response {
    if file == inner.identity.cert_file() {
        (
            [(header::CONTENT_TYPE, "application/x-x509-ca-cert")],
            inner.identity.cert_pem().to_owned(),
        )
            .into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// Everything a request must pass before a decision is drawn: the
/// signature, the operation, the body, the configuration set.
fn admit(
    inner: &Inner,
    method: &Method,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    body: &[u8],
    now: chrono::DateTime<Utc>,
) -> Result<SendTextMessageRequest, AwsError> {
    sigv4::verify(
        &sigv4::Expected {
            access_key_id: &inner.config.access_key_id,
            secret_access_key: &inner.config.secret_access_key,
            region: &inner.config.region,
            service: SERVICE,
        },
        method.as_str(),
        uri.path(),
        uri.query(),
        headers,
        body,
        now,
    )
    .map_err(|refusal| AwsError::new(403, refusal.error_type, None, refusal.message))?;
    if headers.get("x-amz-target").and_then(|v| v.to_str().ok()) != Some(SEND_TEXT_MESSAGE) {
        return Err(AwsError::new(
            400,
            "UnknownOperationException",
            None,
            "sms-fake-aws implements SendTextMessage only",
        ));
    }
    let request: SendTextMessageRequest = serde_json::from_slice(body).map_err(|error| {
        AwsError::new(
            400,
            "ValidationException",
            Some("CANNOT_PARSE"),
            error.to_string(),
        )
    })?;
    if let Some(set) = &request.configuration_set_name
        && *set != inner.config.configuration_set
    {
        // `ResourceType` is how the adapter tells a missing configuration
        // set (the account) from a missing sender identity (the message).
        let mut error = AwsError::new(
            400,
            "ResourceNotFoundException",
            None,
            format!("configuration set {set} does not exist"),
        );
        error.resource_type = Some("configuration-set");
        return Err(error);
    }
    Ok(request)
}

fn record_of(request: &SendTextMessageRequest, received_at: chrono::DateTime<Utc>) -> SubmitRecord {
    SubmitRecord {
        received_at,
        destination: request.destination_phone_number.clone(),
        body: request.message_body.clone().unwrap_or_default(),
        origination_identity: request.origination_identity.clone(),
        message_type: request.message_type.clone(),
        configuration_set: request.configuration_set_name.clone(),
        time_to_live: request.time_to_live,
        context: request.context.clone(),
        message_id: None,
        status: 0,
        error_type: None,
    }
}

fn refuse(inner: &Inner, mut record: SubmitRecord, error: AwsError) -> Response {
    record.status = error.status;
    record.error_type = Some(error.error_type.clone());
    inner.ledger.record_submit(record);
    tracing::warn!(
        status = error.status,
        error_type = error.error_type,
        message = error.message,
        "refused a SendTextMessage call"
    );
    error.into_response()
}

async fn send_text_message(
    State(inner): State<Arc<Inner>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let received_at = Utc::now();
    let request = match admit(&inner, &method, &uri, &headers, &body, received_at) {
        Ok(request) => request,
        Err(error) => {
            let unparsed = SubmitRecord {
                received_at,
                ..SubmitRecord::default()
            };
            return refuse(&inner, unparsed, error);
        }
    };
    let mut record = record_of(&request, received_at);

    let Some(decision) = inner.policy.next() else {
        tracing::error!(
            "sms-fake-aws: the scripted policy has no decision left for this submit; \
             the test submitted more often than it scripted"
        );
        return refuse(
            &inner,
            record,
            AwsError::new(
                500,
                "InternalServerException",
                None,
                "sms-fake-aws: scripted policy exhausted",
            ),
        );
    };

    let response = match &decision.response {
        SubmitResponse::Error {
            status,
            error_type,
            reason,
            message,
        } => refuse(
            &inner,
            record,
            AwsError::new(*status, error_type, reason.as_deref(), message.clone()),
        ),
        accepted => {
            let message_id = uuid_like();
            record.message_id = Some(message_id.clone());
            record.status = 200;
            inner.ledger.record_submit(record);
            tracing::info!(
                message_id,
                destination = request.destination_phone_number,
                events = decision.events.len(),
                "accepted a SendTextMessage call"
            );
            if request.configuration_set_name.is_none() && !decision.events.is_empty() {
                tracing::warn!(
                    message_id,
                    "no ConfigurationSetName: AWS publishes no events for this message, and neither does the fake"
                );
            } else {
                schedule_events(&inner, &request, &message_id, received_at, &decision.events);
            }
            let body = if *accepted == SubmitResponse::Accept {
                json!({ "MessageId": message_id })
            } else {
                json!({})
            };
            ([(header::CONTENT_TYPE, JSON_1_0)], body.to_string()).into_response()
        }
    };
    if !decision.response_delay.is_zero() {
        tokio::time::sleep(decision.response_delay).await;
    }
    response
}

fn schedule_events(
    inner: &Arc<Inner>,
    request: &SendTextMessageRequest,
    message_id: &str,
    requested_at: chrono::DateTime<Utc>,
    events: &[EventStep],
) {
    for step in events {
        let event = json!({
            "eventType": step.event_type,
            "eventVersion": "1.0",
            "eventTimestamp": requested_at.timestamp_millis()
                + i64::try_from(step.after.as_millis()).unwrap_or(i64::MAX),
            "isFinal": step.is_final,
            "originationPhoneNumber": request.origination_identity,
            "destinationPhoneNumber": request.destination_phone_number,
            "isInternationalSend": false,
            "messageId": message_id,
            "messageRequestTimestamp": requested_at.timestamp_millis(),
            "messageEncoding": "GSM",
            "messageType": request.message_type.as_deref().unwrap_or("TRANSACTIONAL"),
            "messageStatus": step.event_type.trim_start_matches("TEXT_"),
            "messageStatusDescription": "sms-fake-aws",
            "context": request.context,
            "totalMessageParts": 1,
            "totalMessagePrice": 0.0,
            "totalCarrierFee": 0.0,
        });
        inner.ledger.notification_scheduled();
        let inner = inner.clone();
        let step = step.clone();
        tokio::spawn(async move {
            tokio::time::sleep(step.after).await;
            inner.publish(event, step).await;
        });
    }
}

fn fresh_id_and_timestamp() -> (String, String) {
    (
        uuid_like(),
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::Rng::fill(&mut rand::thread_rng(), buf.as_mut_slice());
    hex::encode(buf)
}

/// A random UUID-shaped id, as AWS's `MessageId`s are.
fn uuid_like() -> String {
    let hex = random_hex(16);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}
