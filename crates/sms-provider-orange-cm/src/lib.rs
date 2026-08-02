//! [`SmsProvider`] for Orange Cameroon's SMS Cameroon 2.0 / on-net HTTP API.
//! §6.2 of the design doc — "build this first": genuinely self-service,
//! start sending within the hour of registering on the Orange developer
//! portal.
//!
//! **Ceilings that shape this crate, not just document it:** a hard 5 TPS
//! cap and a 100k FCFA/day SIM cap ceiling throughput at roughly 5,000
//! SMS/day (#31). Nothing here enforces either — that is `dispatch`'s
//! `budget` parameter to the claim loop (§7.3, `crates/sms-worker`), which
//! this adapter has no visibility into. [`OrangeCmProvider::capabilities`]
//! reports [`Capabilities::tps_ceiling`] as data for the caller to enforce,
//! not a limit this crate self-polices.
//!
//! Two things in here are transcribed directly from §6.2 and verified only
//! by rereading the doc precisely, not against a live Orange sandbox (this
//! repo has no Orange Developer credentials): the OAuth token endpoint and
//! TTL handling ([`token`]), and the submit request/response shape below.
//! The submit body's `receiptRequest` (`notifyURL`/`callbackData`, #95) is
//! one step further still — §6.2 doesn't mention it at all; it's grounded
//! in the public `OneAPI` SMS Messaging REST binding this whole shape
//! belongs to, not this repo's own design doc. The DLR callback shape
//! ([`dlr`]) is the same distance from §6.2 — see that module's doc.

mod dlr;
mod token;

use async_trait::async_trait;
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sms_provider::{
    Capabilities, DeliveryUpdate, Health, ProviderError, RawCallback, SmsProvider, SubmitAck,
    SubmitRequest,
};

const KEY: &str = "orange_cm";

/// §6.2's stated ceilings, as data rather than prose. `dispatch` reads this
/// through [`SmsProvider::capabilities`] to size its claim budget; this
/// crate does not enforce the TPS cap itself.
///
/// A function, not a `const`: `Decimal::new` isn't `const fn`, and the
/// struct is cheap enough (six `Copy` fields) that rebuilding it per call
/// costs nothing worth optimising away.
fn capabilities() -> Capabilities {
    Capabilities {
        dlr: true,
        // Sender name whitelisted via a support form; max 11 alphanumeric
        // chars plus spaces — support*ed*, not unregistered-free-text.
        alphanumeric_sender: true,
        ucs2: true,
        concatenation: true,
        tps_ceiling: 5.0,
        // SMS Cameroon 2.0 (all-operator) pricing, the product this adapter
        // uses today — see the module doc on on-net product selection being
        // deferred. Middle of §6.2's ~16-22 FCFA quoted range; a real value
        // belongs in `Provider.costPerSegmentXaf` once a contract is
        // signed, not hardcoded here — this is a reasonable placeholder for
        // capability reporting before that exists.
        cost_per_segment_xaf: Decimal::new(19, 0),
    }
}

/// What this adapter needs to talk to Orange. Never a secret in the
/// database (§2.4: `Provider.credentialRef` is a pointer) — the worker
/// resolves `client_id`/`client_secret` at startup and constructs this.
#[derive(Debug, Clone)]
pub struct OrangeCmConfig {
    /// The `OAuth2` `client_credentials` client ID.
    pub client_id: String,
    /// The `OAuth2` `client_credentials` client secret.
    pub client_secret: String,
    /// Cameroon's country sender number, E.164 without the `tel:` scheme —
    /// `"+2370000"`. This crate adds the scheme and percent-encoding.
    pub sender_number: String,
    /// `https://api.orange.com` in production; overridable so tests can
    /// point this at a local mock server instead of the real API.
    pub base_url: String,
    /// `receiptRequest.notifyURL` on every submit (§95's fix — see
    /// `dlr.rs`'s module doc). `None` by default: the module doc's own
    /// long-standing note is that Orange's real DLR webhook is "whitelisted
    /// per a manual support ticket," which reads as a pre-registered,
    /// account-level target rather than a per-request one — so
    /// `callbackData` (always sent) may be all that's actually needed.
    /// This is here as an explicit knob in case a real Orange sandbox says
    /// otherwise, not because its necessity is confirmed.
    pub dlr_notify_url: Option<String>,
}

impl OrangeCmConfig {
    /// Orange's real API host. The only thing most callers need to set
    /// besides credentials.
    #[must_use]
    pub fn production(client_id: String, client_secret: String, sender_number: String) -> Self {
        Self {
            client_id,
            client_secret,
            sender_number,
            base_url: "https://api.orange.com".to_owned(),
            dlr_notify_url: None,
        }
    }
}

/// Orange Cameroon: `OAuth2` token acquisition, SMS submission, DLR parsing.
pub struct OrangeCmProvider {
    client: reqwest::Client,
    config: OrangeCmConfig,
    token: token::TokenCache,
}

impl OrangeCmProvider {
    /// Build an adapter from `config`. Cheap and synchronous — the first
    /// real network call happens on the first [`SmsProvider::submit`] or
    /// [`SmsProvider::health`], not here.
    ///
    /// The client is built with explicit timeouts — `reqwest::Client::new()`
    /// sets none, so a connection that hangs (Orange accepts the TCP
    /// handshake but never responds) would otherwise stall `submit`
    /// indefinitely. Once `dispatch` (§7.1, still a stub) holds this
    /// provider, an indefinite hang there is an indefinite hang of the
    /// worker's claim loop, not just one message.
    ///
    /// # Panics
    ///
    /// Never in practice — the only way `ClientBuilder::build` fails is a
    /// TLS backend that can't initialise, and this crate's `rustls-tls`
    /// feature has no such failure mode with only timeouts configured.
    #[must_use]
    pub fn new(mut config: OrangeCmConfig) -> Self {
        // `submit_url`/`submit` each add the `tel:` scheme themselves; a
        // caller who already included it (the field is public, so
        // `production()` isn't the only way to set it) would otherwise get
        // `tel:tel:+2370000` on every request — rejected by Orange with no
        // hint that the scheme was the problem. Stripped once here rather
        // than validated and rejected, since a doubled scheme is always a
        // caller mistake, never a legitimate value.
        // clippy's own `clone_into` suggestion doesn't compile here: `stripped`
        // borrows from `config.sender_number`, so writing back into that same
        // field through it is E0502, not just a style preference.
        #[allow(clippy::assigning_clones)]
        if let Some(stripped) = config.sender_number.strip_prefix("tel:") {
            config.sender_number = stripped.to_owned();
        }
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client builder with only timeouts set never fails"),
            config,
            token: token::TokenCache::new(),
        }
    }

    /// A valid bearer token, fetching and caching a fresh one if the cached
    /// one has passed its 80%-of-lifetime refresh margin (§6.2, `token`'s
    /// module doc).
    ///
    /// No in-flight gate on a refresh: N concurrent `submit`/`health` calls
    /// that all observe an expired token each fetch and store their own —
    /// `TokenCache::store` makes the last write win, and every fetched
    /// token is independently valid, so this is a burst of redundant
    /// requests, not a correctness bug. Flagged in review (#94). Bounded by
    /// the adapter's own 5 TPS ceiling (`capabilities().tps_ceiling`), so
    /// the worst case is a handful of extra token requests at each ~hourly
    /// refresh boundary — not worth a `tokio::sync::Mutex`/`OnceCell`
    /// refresh gate until real traffic says otherwise.
    async fn access_token(&self) -> Result<String, ProviderError> {
        if let Some(token) = self.token.valid() {
            return Ok(token);
        }

        let token_url = format!("{}/oauth/v3/token", self.config.base_url);
        let fetched = token::fetch(
            &self.client,
            &token_url,
            &self.config.client_id,
            &self.config.client_secret,
        )
        .await?;

        self.token.store(
            fetched.access_token.clone(),
            std::time::Duration::from_secs(fetched.expires_in),
        );
        Ok(fetched.access_token)
    }

    /// `POST /smsmessaging/v1/outbound/{sender}/requests` — §6.2. Built via
    /// `Url::path_segments_mut` rather than hand-rolled percent-encoding
    /// (`tel%3A%2B2370000`), so the encoding is the standard library's, not
    /// this crate's.
    fn submit_url(&self) -> Result<reqwest::Url, ProviderError> {
        let mut url = reqwest::Url::parse(&self.config.base_url).map_err(|error| {
            ProviderError::Unavailable {
                message: format!("invalid base_url: {error}"),
            }
        })?;
        url.path_segments_mut()
            .map_err(|()| ProviderError::Unavailable {
                message: "base_url cannot be a base for path segments".to_owned(),
            })?
            .extend([
                "smsmessaging",
                "v1",
                "outbound",
                &format!("tel:{}", self.config.sender_number),
                "requests",
            ]);
        Ok(url)
    }
}

#[derive(Debug, Serialize)]
struct OutboundSmsTextMessage<'a> {
    message: &'a str,
}

/// #95's fix: `callback_data` is always `SubmitRequest::reference`
/// (`Message.id`) so the delivery notification can echo it back as the
/// correlation key `dlr.rs::parse` reads — see that module's doc for the
/// full reasoning and the public `OneAPI` reference this is grounded in.
/// `notify_url` is omitted from the request entirely when unset
/// (`OrangeCmConfig::dlr_notify_url` is `None` by default) rather than sent
/// as `null` — `skip_serializing_if` here, not an `Option` Orange has to
/// specially handle.
#[derive(Debug, Serialize)]
struct ReceiptRequest<'a> {
    #[serde(rename = "notifyURL", skip_serializing_if = "Option::is_none")]
    notify_url: Option<&'a str>,
    #[serde(rename = "callbackData")]
    callback_data: &'a str,
}

#[derive(Debug, Serialize)]
struct OutboundSmsMessageBody<'a> {
    address: Vec<String>,
    #[serde(rename = "senderAddress")]
    sender_address: String,
    #[serde(rename = "senderName")]
    sender_name: &'a str,
    #[serde(rename = "outboundSMSTextMessage")]
    outbound_sms_text_message: OutboundSmsTextMessage<'a>,
    #[serde(rename = "receiptRequest")]
    receipt_request: ReceiptRequest<'a>,
}

#[derive(Debug, Serialize)]
struct OutboundSmsRequest<'a> {
    #[serde(rename = "outboundSMSMessageRequest")]
    outbound_sms_message_request: OutboundSmsMessageBody<'a>,
}

#[derive(Debug, Deserialize)]
struct SubmitResponseEnvelope {
    #[serde(rename = "outboundSMSMessageRequest")]
    outbound_sms_message_request: SubmitResponseBody,
}

#[derive(Debug, Deserialize)]
struct SubmitResponseBody {
    #[serde(rename = "resourceReference")]
    resource_reference: ResourceReference,
}

#[derive(Debug, Deserialize)]
struct ResourceReference {
    #[serde(rename = "resourceURL")]
    resource_url: String,
}

/// The trailing path segment of a `resourceURL` is the `resource_id` —
/// §6.2: "`201` + a `resource_id` UUID is your DLR correlation key."
fn resource_id_from_url(resource_url: &str) -> Option<&str> {
    resource_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
}

#[async_trait]
impl SmsProvider for OrangeCmProvider {
    fn key(&self) -> &str {
        KEY
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn submit(&self, req: &SubmitRequest) -> Result<SubmitAck, ProviderError> {
        let token = self.access_token().await?;
        let url = self.submit_url()?;

        let body = OutboundSmsRequest {
            outbound_sms_message_request: OutboundSmsMessageBody {
                address: vec![format!("tel:{}", req.to)],
                sender_address: format!("tel:{}", self.config.sender_number),
                sender_name: &req.sender_id,
                outbound_sms_text_message: OutboundSmsTextMessage { message: &req.body },
                receipt_request: ReceiptRequest {
                    notify_url: self.config.dlr_notify_url.as_deref(),
                    callback_data: &req.reference,
                },
            },
        };

        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|error| ProviderError::Unavailable {
                message: format!("submit request failed: {error}"),
            })?;

        let status = response.status();
        if status != StatusCode::CREATED {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_submit_error(status, &text));
        }

        let parsed: SubmitResponseEnvelope =
            response
                .json()
                .await
                .map_err(|error| ProviderError::Unavailable {
                    message: format!("submit response was not valid JSON: {error}"),
                })?;

        let resource_url = parsed
            .outbound_sms_message_request
            .resource_reference
            .resource_url;
        let provider_ref = resource_id_from_url(&resource_url)
            .ok_or_else(|| ProviderError::Unavailable {
                message: format!("no resource id in resourceURL {resource_url:?}"),
            })?
            .to_owned();

        Ok(SubmitAck {
            provider_ref,
            // #95's fix, completed: `sms_api::dlr::ingest_one` matches a
            // DLR's `provider_ref` against `Message.providerMessageRef` OR
            // `providerMessageRefAlt` — generic, provider-agnostic
            // correlation built on the assumption that whatever a
            // `DeliveryUpdate::provider_ref` holds was already stored in
            // one of those two columns at submit time. `provider_ref`
            // above (the resource_id) satisfies that for
            // `providerMessageRef`, but the DLR's own `provider_ref` is
            // now `callbackData` (`req.reference`, i.e. `Message.id` —
            // see the `receiptRequest` above), a *different* value. Found
            // live dry-running #36: without this, a synthetic DLR with the
            // exact `callbackData` the request itself carried still logged
            // "no known message" and matched nothing.
            // `SubmitAck::provider_ref_alt` exists precisely for this
            // shape — "a provider reports the same submission two
            // different ways, once at submit time, once at DLR time" — so
            // this is that second form, not a new field or a change to
            // `sms-api`'s generic matching logic.
            provider_ref_alt: Some(req.reference.clone()),
        })
    }

    fn parse_dlr(&self, raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
        dlr::parse(raw)
    }

    async fn health(&self) -> Health {
        match self.access_token().await {
            Ok(_) => Health::ok(),
            Err(error) => Health::unhealthy(error.to_string()),
        }
    }
}

/// §6.2: 429 is the 5 TPS ceiling being hit — transient, retry, do not fail
/// over. 5xx is Orange's own backend — unavailable, fail over and count
/// toward the circuit breaker. Everything else 4xx, conservatively,
/// `Rejected`: §6.2 names one specific 400 cause (an unapproved sender
/// name) but not its error body shape, so this crate cannot yet
/// distinguish "this destination is bad" from "this sender ID needs a
/// different provider" — see the module doc. `Rejected` is the safer
/// default of the two: it fails this one message rather than risking a
/// failover storm across every provider for a systematically bad sender ID.
fn classify_submit_error(status: StatusCode, body: &str) -> ProviderError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return ProviderError::Transient {
            // Orange doesn't document a Retry-After for this endpoint;
            // one full TPS-cap window is a reasonable, conservative wait.
            retry_after: std::time::Duration::from_secs(1),
            message: format!("rate limited: {body}"),
        };
    }
    if status.is_server_error() {
        return ProviderError::Unavailable {
            message: format!("orange returned {status}: {body}"),
        };
    }
    ProviderError::Rejected {
        code: format!("http_{}", status.as_u16()),
        message: body.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{resource_id_from_url, OrangeCmConfig, OrangeCmProvider, KEY};
    use sms_encoding::SmsEncoding;
    use sms_provider::{ProviderError, SmsProvider, SubmitRequest};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn provider(base_url: String) -> OrangeCmProvider {
        OrangeCmProvider::new(OrangeCmConfig {
            client_id: "client".to_owned(),
            client_secret: "secret".to_owned(),
            sender_number: "+2370000".to_owned(),
            base_url,
            dlr_notify_url: None,
        })
    }

    async fn mock_token_endpoint(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/oauth/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "test-token",
                "expires_in": 3600,
            })))
            .mount(server)
            .await;
    }

    #[test]
    fn key_is_the_providers_schema_key() {
        assert_eq!(provider(String::new()).key(), KEY);
    }

    #[test]
    fn a_sender_number_that_already_has_the_tel_scheme_is_not_doubled() {
        let with_scheme = OrangeCmProvider::new(OrangeCmConfig {
            client_id: "client".to_owned(),
            client_secret: "secret".to_owned(),
            sender_number: "tel:+2370000".to_owned(),
            base_url: "https://example.invalid".to_owned(),
            dlr_notify_url: None,
        });
        let without_scheme = provider("https://example.invalid".to_owned());

        assert_eq!(
            with_scheme.submit_url().unwrap(),
            without_scheme.submit_url().unwrap(),
            "a caller-supplied `tel:` prefix must not produce `tel:tel:...`"
        );
    }

    #[test]
    fn extracts_the_resource_id_from_a_resource_url() {
        assert_eq!(
            resource_id_from_url(
                "https://api.orange.com/smsmessaging/v1/outbound/tel:+2370000/requests/abc-123"
            ),
            Some("abc-123")
        );
        assert_eq!(resource_id_from_url("no-slashes"), Some("no-slashes"));
        assert_eq!(resource_id_from_url("trailing/slash/"), Some("slash"));
        assert_eq!(resource_id_from_url(""), None);
    }

    #[tokio::test]
    async fn submit_succeeds_and_extracts_the_resource_id() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "outboundSMSMessageRequest": {
                    "resourceReference": {
                        "resourceURL": "https://api.orange.com/.../requests/res-42"
                    }
                }
            })))
            .mount(&server)
            .await;

        let ack = provider(server.uri())
            .submit(&SubmitRequest {
                to: "+237677123456".to_owned(),
                sender_id: "VYMALO".to_owned(),
                body: "Votre code est 4821".to_owned(),
                encoding: SmsEncoding::Gsm7,
                reference: "msg-1".to_owned(),
            })
            .await
            .unwrap();

        assert_eq!(ack.provider_ref, "res-42");
        assert_eq!(
            ack.provider_ref_alt,
            Some("msg-1".to_owned()),
            "the caller's own reference (== callbackData) must be the alt reference, so \
             sms-api's generic providerMessageRefAlt matching finds the DLR — see submit()'s \
             own doc"
        );
    }

    /// #95's actual fix, proven at the wire level: the request Orange
    /// receives carries `receiptRequest.callbackData` set to the caller's
    /// own `reference` — this is what `dlr.rs::parse` reads back as
    /// `provider_ref` once Orange echoes it in a delivery notification.
    #[tokio::test]
    async fn submit_sets_receipt_request_callback_data_to_the_reference() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "outboundSMSMessageRequest": {
                    "receiptRequest": {"callbackData": "msg-77"}
                }
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "outboundSMSMessageRequest": {
                    "resourceReference": {"resourceURL": "https://x/res-77"}
                }
            })))
            .mount(&server)
            .await;

        let ack = provider(server.uri())
            .submit(&SubmitRequest {
                to: "+237677123456".to_owned(),
                sender_id: "VYMALO".to_owned(),
                body: "hi".to_owned(),
                encoding: SmsEncoding::Gsm7,
                reference: "msg-77".to_owned(),
            })
            .await
            .unwrap();

        assert_eq!(ack.provider_ref, "res-77");
    }

    /// `notifyURL` is caller-configured and off by default
    /// (`OrangeCmConfig::dlr_notify_url: None`) — when unset, it must not
    /// appear in the request at all (`skip_serializing_if`, not a JSON
    /// `null` Orange would have to specially tolerate). Inspects the
    /// actual recorded request body rather than a matcher, since a
    /// negative-presence matcher would only prove some other mock didn't
    /// match, not that the real one lacks the field.
    #[tokio::test]
    async fn submit_omits_notify_url_when_unconfigured() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "outboundSMSMessageRequest": {
                    "resourceReference": {"resourceURL": "https://x/res-78"}
                }
            })))
            .mount(&server)
            .await;

        provider(server.uri())
            .submit(&SubmitRequest {
                to: "+237677123456".to_owned(),
                sender_id: "VYMALO".to_owned(),
                body: "hi".to_owned(),
                encoding: SmsEncoding::Gsm7,
                reference: "msg-78".to_owned(),
            })
            .await
            .unwrap();

        let requests = server.received_requests().await.expect("recording enabled");
        let submit_request = requests
            .iter()
            .find(|r| r.url.path().contains("/requests"))
            .expect("the submit request was recorded");
        let body: serde_json::Value = submit_request.body_json().unwrap();
        let receipt_request = &body["outboundSMSMessageRequest"]["receiptRequest"];
        assert_eq!(receipt_request["callbackData"], "msg-78");
        assert!(
            receipt_request.get("notifyURL").is_none(),
            "notifyURL must be absent, not null, when unconfigured: {receipt_request}"
        );
    }

    #[tokio::test]
    async fn submit_reuses_a_cached_token_across_two_calls() {
        let server = MockServer::start().await;
        // .expect(1): if submit fetched a token per message (the exact
        // thing §6.2 says not to do), this mock would see a second
        // request and the test would fail on drop.
        Mock::given(method("POST"))
            .and(path("/oauth/v3/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "test-token",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "outboundSMSMessageRequest": {
                    "resourceReference": {"resourceURL": "https://x/res-1"}
                }
            })))
            .mount(&server)
            .await;

        let provider = provider(server.uri());
        let request = |n: u32| SubmitRequest {
            to: "+237677123456".to_owned(),
            sender_id: "VYMALO".to_owned(),
            body: format!("message {n}"),
            encoding: SmsEncoding::Gsm7,
            reference: format!("msg-{n}"),
        };

        provider.submit(&request(1)).await.unwrap();
        provider.submit(&request(2)).await.unwrap();
    }

    #[tokio::test]
    async fn a_rate_limited_submit_is_transient() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&SubmitRequest {
                to: "+237677123456".to_owned(),
                sender_id: "VYMALO".to_owned(),
                body: "hi".to_owned(),
                encoding: SmsEncoding::Gsm7,
                reference: "msg-1".to_owned(),
            })
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Transient { .. }));
    }

    #[tokio::test]
    async fn a_server_error_submit_is_unavailable() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&SubmitRequest {
                to: "+237677123456".to_owned(),
                sender_id: "VYMALO".to_owned(),
                body: "hi".to_owned(),
                encoding: SmsEncoding::Gsm7,
                reference: "msg-1".to_owned(),
            })
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Unavailable { .. }));
    }

    #[tokio::test]
    async fn an_unapproved_sender_400_is_rejected() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(400).set_body_string("sender name not approved"))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&SubmitRequest {
                to: "+237677123456".to_owned(),
                sender_id: "UNAPPROVED".to_owned(),
                body: "hi".to_owned(),
                encoding: SmsEncoding::Gsm7,
                reference: "msg-1".to_owned(),
            })
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Rejected { .. }));
    }

    #[tokio::test]
    async fn bad_credentials_at_the_token_endpoint_are_permanent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/v3/token"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&SubmitRequest {
                to: "+237677123456".to_owned(),
                sender_id: "VYMALO".to_owned(),
                body: "hi".to_owned(),
                encoding: SmsEncoding::Gsm7,
                reference: "msg-1".to_owned(),
            })
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Permanent { .. }));
    }

    #[tokio::test]
    async fn health_is_ok_when_a_token_can_be_obtained() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        assert!(provider(server.uri()).health().await.healthy);
    }

    #[tokio::test]
    async fn health_is_unhealthy_when_the_token_endpoint_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/v3/token"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let health = provider(server.uri()).health().await;
        assert!(!health.healthy);
        assert!(health.detail.is_some());
    }
}
