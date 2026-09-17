#![doc = include_str!("lib.md")]

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
use sms_provider_http::{classify_common_submit_status, classify_transport_error};

/// The noun `classify_transport_error`'s `Indeterminate` message and
/// `classify_submit_error`'s `Unavailable` message both name this provider
/// as — preserved verbatim from this crate's own pre-consolidation text
/// (`"Orange may have received it"`, `"orange returned {status}"`; note
/// the case difference between the two is original, not a typo introduced
/// here — see `sms-provider-http`'s own module doc on why it was kept
/// rather than normalised).
const PROVIDER_NOUN_INDETERMINATE: &str = "Orange";
const PROVIDER_NOUN_UNAVAILABLE: &str = "orange";

/// §6.2 names no `Retry-After` for this endpoint; this is unchanged from
/// this crate's own pre-consolidation constant.
const RATE_LIMIT_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(1);

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
///
/// Deliberately carries no `notify_url`/DLR-webhook field. Orange's own
/// docs (§4 "About SMS Delivery Receipt",
/// <https://developer.orange.com/apis/sms/getting-started>) are explicit
/// that the DR endpoint "must be whitelisted on the Orange SMS API server
/// before being used; to do this, please fill in this web form" — an
/// account-level, pre-registered target, not something a per-request field
/// on this struct could ever express. There used to be a `dlr_notify_url`
/// knob here, plumbed into a per-request `receiptRequest.notifyURL` — that
/// was speculative (§6.2 never mentioned `receiptRequest` at all; it was
/// inferred from the wider GSMA `OneAPI` family this adapter otherwise
/// follows) and is gone now that the real docs show no such request field
/// exists. See `lib.md` and `dlr.rs`'s own module doc for the full
/// correlation story.
#[derive(Clone)]
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
    /// TCP/TLS connect timeout for [`OrangeCmProvider::new`]'s client.
    /// `production()` sets this to 10s. Exposed as a knob (rather than
    /// hardcoded in `new`) so a live test can shrink it and prove the
    /// connect-vs-read-timeout distinction in `classify_transport_error`
    /// deterministically, without waiting on the production value.
    pub connect_timeout: std::time::Duration,
    /// Overall request timeout — connect *and* send *and* await the
    /// response — for [`OrangeCmProvider::new`]'s client. `production()`
    /// sets this to 30s. Same testing rationale as `connect_timeout`: a
    /// wiremock response delayed past this value is what produces a
    /// genuine, deterministic read timeout.
    pub request_timeout: std::time::Duration,
}

impl std::fmt::Debug for OrangeCmConfig {
    /// Hand-written, not derived — `client_secret` must never reach a
    /// `{:?}` log line. Same discipline as `sms_api::pepper::HashPepper`
    /// and the Rust SDK's own `PrivateKeyJwtConfig`; this struct was the
    /// one config type in the workspace still holding real credential
    /// material behind a derived `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrangeCmConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("sender_number", &self.sender_number)
            .field("base_url", &self.base_url)
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
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
            connect_timeout: std::time::Duration::from_secs(10),
            request_timeout: std::time::Duration::from_secs(30),
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
    /// The client is built with explicit timeouts, from `config` —
    /// `reqwest::Client::new()` sets none, so a connection that hangs
    /// (Orange accepts the TCP handshake but never responds) would
    /// otherwise stall `submit` indefinitely. Once `dispatch` (§7.1, still
    /// a stub) holds this provider, an indefinite hang there is an
    /// indefinite hang of the worker's claim loop, not just one message.
    /// See `classify_transport_error` for why *which* of these two
    /// timeouts fires changes whether a retry is safe.
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
                .connect_timeout(config.connect_timeout)
                .timeout(config.request_timeout)
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
                source: Some(Box::new(error)),
            }
        })?;
        url.path_segments_mut()
            .map_err(|()| ProviderError::Unavailable {
                message: "base_url cannot be a base for path segments".to_owned(),
                // `()` on the `Err` side — `Url::path_segments_mut` reports
                // failure with no error object at all, so there is nothing
                // real to chain here.
                source: None,
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

/// Matches Orange's own documented submit body exactly
/// (<https://developer.orange.com/apis/sms/getting-started>, the "Getting
/// Started" sample request): `address`, `senderAddress`, `senderName`,
/// `outboundSMSTextMessage` — nothing else. There used to be a fifth
/// field, `receiptRequest` (`notifyURL`/`callbackData`), sending
/// `Message.id` as a caller-supplied correlation token — that was never
/// in §6.2 or the real docs to begin with (it was inferred from the wider
/// GSMA `OneAPI` family this adapter's shape otherwise follows), and the
/// documented body has no such field at all: correlation is Orange's own
/// `resource_id`, reported at submit time in `resourceURL` and again at
/// DR time in `callbackData` — see `dlr.rs`'s own module doc and
/// `resource_id_from_url` below. The maintainer's explicit decision:
/// stop sending a field Orange's docs don't describe, and accept the
/// consequence that a submit whose response is never read
/// (`ProviderError::Indeterminate`) is now permanently uncorrelatable —
/// see `submit()`'s own doc below, on the `SubmitAck { provider_ref_alt,
/// .. }` construction, for the full reasoning.
#[derive(Debug, Serialize)]
struct OutboundSmsMessageBody<'a> {
    /// A JSON **array**, even for the single recipient this adapter always
    /// sends to — and deliberately not "corrected" to the bare string
    /// Orange's own getting-started examples show.
    ///
    /// This looks like a discrepancy against the docs and is the obvious
    /// next thing to change after the `deliveryInfo`-object fix in
    /// `dlr.rs`. Don't, without evidence. The GSMA `OneAPI` shape Orange's
    /// API belongs to defines `address` as a list precisely because a
    /// request may carry several recipients; Orange's docs simply show the
    /// one-recipient case in its simplest form. The array form is what
    /// this adapter has always sent, and #389 corrected the *response*
    /// parser against a genuinely observed Orange `201` body — which means
    /// a real submit carrying this array was accepted, and the endpoint
    /// got far enough to answer with a `resourceURL`.
    ///
    /// That is evidence, not proof (nobody recorded the request that
    /// produced it), so confirm it alongside everything else the first
    /// time `docs/runbooks/36-handset-gate.adoc` is actually run. If a
    /// real submit ever returns `400` on a well-formed body, this field is
    /// the first thing to try as a bare string — and record the answer
    /// here rather than leaving the next person to re-derive it.
    address: Vec<String>,
    #[serde(rename = "senderAddress")]
    sender_address: String,
    #[serde(rename = "senderName")]
    sender_name: &'a str,
    #[serde(rename = "outboundSMSTextMessage")]
    outbound_sms_text_message: OutboundSmsTextMessage<'a>,
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

/// Orange's real 201 body (captured live, #95) carries `resourceURL` DIRECTLY
/// inside `outboundSMSMessageRequest` — NOT nested under a `resourceReference`
/// wrapper as the public `OneAPI` docs describe. Verified against a real
/// `201 Created` response:
/// `{"outboundSMSMessageRequest":{...,"resourceURL":"<host>/.../requests/<uuid>"}}`.
#[derive(Debug, Deserialize)]
struct SubmitResponseBody {
    #[serde(rename = "resourceURL")]
    resource_url: String,
}

/// The trailing path segment of a `resourceURL` is the `resource_id` — §4
/// "About SMS Delivery Receipt"
/// (<https://developer.orange.com/apis/sms/getting-started>) names it as
/// "a string with the following *typical* format:
/// xxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" (emphasis on "typical" — Orange's
/// own docs deliberately don't promise a fixed shape, so this function
/// stays format-agnostic on purpose: no length check, no UUID parse, no
/// assumption about hyphens or character set. It just takes whatever the
/// last `/`-delimited segment is, exactly as it would for any opaque
/// resource id. This is Orange's own `{{resource_id}}` — the DLR
/// correlation key §4 describes, not a caller-supplied value.
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
            },
        };

        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|error| classify_transport_error(error, PROVIDER_NOUN_INDETERMINATE))?;

        let status = response.status();
        if status != StatusCode::CREATED {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_submit_error(status, &text));
        }

        // From here down, Orange already answered 201 — the submission was
        // accepted. Any failure past this point (an unparseable body, a
        // missing resourceURL) means we cannot recover the provider_ref to
        // store, but we know for a fact the SMS may already be in flight,
        // so `Indeterminate` applies, not `Unavailable`: retrying this
        // "failed" submit would risk sending it a second time.
        let parsed: SubmitResponseEnvelope =
            response
                .json()
                .await
                .map_err(|error| ProviderError::Indeterminate {
                    message: format!(
                        "submit returned 201 (accepted) but the response body was not valid \
                         JSON, so no provider_ref could be recovered: {error}"
                    ),
                    source: Some(Box::new(error)),
                })?;

        let resource_url = parsed.outbound_sms_message_request.resource_url;
        let provider_ref = resource_id_from_url(&resource_url)
            .ok_or_else(|| ProviderError::Indeterminate {
                message: format!(
                    "submit returned 201 (accepted) but no resource id was found in \
                     resourceURL {resource_url:?}"
                ),
                // No real error object — the body parsed as valid JSON,
                // it just lacked the field this adapter needed.
                source: None,
            })?
            .to_owned();

        Ok(SubmitAck {
            provider_ref,
            // Orange reports exactly one id for a submission — its own
            // `resource_id` (§4 "About SMS Delivery Receipt"), once at
            // submit time (`resourceURL`, extracted above) and again at DR
            // time (`callbackData`, `dlr.rs::parse`). Both values are the
            // same `resource_id`, so one column (`Message.providerMessageRef`)
            // is all correlation ever needs — `provider_ref_alt` stays
            // `None` for this provider.
            //
            // This is a deliberate, documented reversal of #95's original
            // fix, which used `provider_ref_alt` to carry `req.reference`
            // (`Message.id`) as a second, caller-chosen correlation value,
            // sent to Orange as `receiptRequest.callbackData`. That field
            // never existed in Orange's real, documented submit request —
            // it was inferred from the wider GSMA `OneAPI` family this
            // adapter's shape otherwise follows, never confirmed against
            // §6.2 or Orange's own docs. The maintainer's decision:
            // stop sending a field Orange doesn't document, and match the
            // real submit body exactly (see `OutboundSmsMessageBody`'s own
            // doc).
            //
            // The accepted consequence: a submit whose `201` response is
            // never successfully read (`ProviderError::Indeterminate`,
            // above) never learns Orange's `resource_id` at all, so
            // nothing is ever stored in `providerMessageRef` for that
            // attempt — and with no caller-supplied token to fall back on
            // any more, such a message is now permanently
            // uncorrelatable. A DLR Orange eventually sends for it (with
            // the real `resource_id` this system never learned) matches
            // nothing and is dropped; `Message` stays `uncertain` until
            // `backends/crates/sms-worker/src/jobs/expire_stale.rs`'s own
            // 6h grace reaps it. This is right for OTP-class traffic
            // (§4.6/§8's own framing, echoed in `ProviderError`'s own
            // doc): a possibly-lost message is a better failure than a
            // possible duplicate send, and there is no way to have both
            // "match Orange's documented request shape exactly" and
            // "keep a caller-chosen correlation token Orange never
            // promised to honour."
            provider_ref_alt: None,
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
///
/// All three of those mappings turned out identical to `sms-provider-mtn`'s
/// own (see `sms-provider-http::classify_common_submit_status`'s module
/// doc), so this function is now a thin, Orange-specific wrapper rather
/// than a second copy of the mapping itself: Orange has no status this
/// crate treats as a special case ahead of the shared mapping (unlike
/// MTN's `401`/`403` — see that crate's own `classify_submit_error`), so
/// every status this function was ever asked to classify already falls
/// straight through to the shared function.
fn classify_submit_error(status: StatusCode, body: &str) -> ProviderError {
    classify_common_submit_status(
        status,
        body,
        PROVIDER_NOUN_UNAVAILABLE,
        RATE_LIMIT_RETRY_AFTER,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        KEY, OrangeCmConfig, OrangeCmProvider, PROVIDER_NOUN_INDETERMINATE,
        classify_transport_error, resource_id_from_url,
    };
    use sms_encoding::SmsEncoding;
    use sms_provider::{ProviderError, SmsProvider, SubmitRequest};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn debug_redacts_the_client_secret() {
        let config = OrangeCmConfig::production(
            "test-client-id".to_owned(),
            "NEVER-PRINT-THIS".to_owned(),
            "+2370000".to_owned(),
        );
        let rendered = format!("{config:?}");
        assert!(
            !rendered.contains("NEVER-PRINT-THIS"),
            "client_secret reached a Debug rendering: {rendered}"
        );
        assert!(rendered.contains("<redacted>"), "{rendered}");
        // The non-secret fields must still be readable — a redacting
        // Debug that hides everything is useless for diagnosis.
        assert!(rendered.contains("test-client-id"), "{rendered}");
        assert!(rendered.contains("+2370000"), "{rendered}");
    }

    /// Generous enough that no existing test (all fast, local wiremock
    /// responses) ever brushes against it, short enough that the
    /// deliberately-slow indeterminate-timeout tests below don't need to
    /// wait anywhere near the 10s/30s production defaults.
    const TEST_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    const TEST_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    fn provider(base_url: String) -> OrangeCmProvider {
        OrangeCmProvider::new(OrangeCmConfig {
            client_id: "client".to_owned(),
            client_secret: "secret".to_owned(),
            sender_number: "+2370000".to_owned(),
            base_url,
            connect_timeout: TEST_CONNECT_TIMEOUT,
            request_timeout: TEST_REQUEST_TIMEOUT,
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
            connect_timeout: TEST_CONNECT_TIMEOUT,
            request_timeout: TEST_REQUEST_TIMEOUT,
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
                    "resourceURL": "https://api.orange.com/.../requests/res-42"
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
            ack.provider_ref_alt, None,
            "Orange reports exactly one id (resource_id) — there is no second, caller-chosen \
             correlation value any more; see submit()'s own doc on why provider_ref_alt is \
             always None for this provider"
        );
    }

    /// The maintainer's decision, proven at the wire level rather than
    /// merely asserted: the submitted body has NO `receiptRequest` field
    /// at all — not an empty object, not present-with-nulls, genuinely
    /// absent. §95's original fix sent `receiptRequest.callbackData` set
    /// to the caller's own `reference`; that field was never in Orange's
    /// real, documented submit request
    /// (<https://developer.orange.com/apis/sms/getting-started>), so this
    /// now asserts the opposite of what it used to.
    #[tokio::test]
    async fn submit_never_sends_a_receipt_request_field() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "outboundSMSMessageRequest": {
                    "resourceURL": "https://x/res-77"
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
                reference: "msg-77".to_owned(),
            })
            .await
            .unwrap();

        let requests = server.received_requests().await.expect("recording enabled");
        let submit_request = requests
            .iter()
            .find(|r| r.url.path().contains("/requests"))
            .expect("the submit request was recorded");
        let body: serde_json::Value = submit_request.body_json().unwrap();
        let inner = &body["outboundSMSMessageRequest"];
        assert!(
            inner.get("receiptRequest").is_none(),
            "the documented submit body has no receiptRequest field at all: {inner}"
        );
    }

    /// The full documented submit body, verbatim — §"Getting Started"'s own
    /// sample request shape
    /// (<https://developer.orange.com/apis/sms/getting-started>): exactly
    /// `address`, `senderAddress`, `senderName`, `outboundSMSTextMessage`,
    /// nothing more and nothing less. `serde_json::Value::eq` on the whole
    /// object catches an unexpected extra field as readily as a missing
    /// one — the shape this test guards against silently regressing is a
    /// field creeping back in, not just `receiptRequest` specifically.
    #[tokio::test]
    async fn submit_body_matches_the_documented_shape_exactly() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "outboundSMSMessageRequest": {
                    "resourceURL": "https://x/res-78"
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
        assert_eq!(
            body,
            serde_json::json!({
                "outboundSMSMessageRequest": {
                    "address": ["tel:+237677123456"],
                    "senderAddress": "tel:+2370000",
                    "senderName": "VYMALO",
                    "outboundSMSTextMessage": {"message": "hi"}
                }
            }),
            "the submitted body must match Orange's own documented shape exactly, no more, no \
             fewer fields"
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
                    "resourceURL": "https://x/res-1"
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

    /// A 2xx response is Orange telling us the submission was accepted —
    /// unparseable body or not. Treating this as `Unavailable` (safe to
    /// retry) would resubmit an SMS Orange has already agreed to send.
    #[tokio::test]
    async fn submit_returns_201_but_an_unparseable_body_is_indeterminate() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(201).set_body_string("not json"))
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

        assert!(
            matches!(error, ProviderError::Indeterminate { .. }),
            "expected Indeterminate, got {error:?}"
        );
    }

    /// Same reasoning as the unparseable-body case, for the narrower
    /// failure of a well-formed 201 body that just doesn't carry a usable
    /// `resourceURL` — still a 201, still an accepted submission.
    #[tokio::test]
    async fn submit_returns_201_but_a_missing_resource_url_is_indeterminate() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/smsmessaging/v1/outbound/tel:+2370000/requests"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "outboundSMSMessageRequest": {
                    "resourceURL": ""
                }
            })))
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

        assert!(
            matches!(error, ProviderError::Indeterminate { .. }),
            "expected Indeterminate, got {error:?}"
        );
    }

    /// Proves `sms_provider_http::classify_transport_error`'s safe branch
    /// *through this adapter's own call site* — with a real
    /// connection-refused error rather than a mocked one: bind an
    /// ephemeral port, then drop the listener so the address is valid but
    /// refuses every connection. Nothing about a submission could have
    /// reached whatever might eventually listen there, so this must stay
    /// exactly as safe to retry as it always was. `sms-provider-http`'s own
    /// crate carries the same proof against the shared function directly
    /// (`AGENTS.md`'s "Cleanup: one transport classifier for every HTTP
    /// adapter" section explains why both levels are kept: this one proves
    /// Orange's own import and provider noun are wired correctly, not just
    /// that the underlying classifier works in isolation).
    #[tokio::test]
    async fn a_connect_refusal_is_still_unavailable() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
        let addr = listener.local_addr().expect("reading the bound address");
        drop(listener);

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("building a plain reqwest client");
        let error = client
            .get(format!("http://{addr}"))
            .send()
            .await
            .expect_err("nothing listens on a dropped ephemeral port");

        assert!(
            error.is_connect(),
            "test setup: expected a connect-level failure, got {error:?}"
        );
        assert!(matches!(
            classify_transport_error(error, PROVIDER_NOUN_INDETERMINATE),
            ProviderError::Unavailable { .. }
        ));
    }

    /// Proves `sms_provider_http::classify_transport_error`'s unsafe branch
    /// *through this adapter's own call site*: a connection that *does*
    /// establish, against a server that then never answers before the
    /// client's own timeout fires. This is exactly the shape a slow/hung
    /// Orange endpoint produces — the request may already be sitting on
    /// Orange's side.
    #[tokio::test]
    async fn a_post_connect_timeout_is_indeterminate() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(
                ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(500)),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_millis(50))
            .build()
            .expect("building a plain reqwest client");
        let error = client
            .get(format!("{}/slow", server.uri()))
            .send()
            .await
            .expect_err("the mock's delay exceeds the client's own timeout");

        assert!(
            error.is_timeout(),
            "test setup: expected a timeout, got {error:?}"
        );
        assert!(
            !error.is_connect(),
            "test setup: the connection must already be established when the timeout fires, \
             or this isn't testing the branch it claims to"
        );
        let classified = classify_transport_error(error, PROVIDER_NOUN_INDETERMINATE);
        assert!(
            matches!(classified, ProviderError::Indeterminate { .. }),
            "expected Indeterminate, got {classified:?}"
        );
        if let ProviderError::Indeterminate { message, .. } = classified {
            assert!(
                message.contains("Orange may have received it"),
                "the provider noun must survive verbatim from before this was shared: {message}"
            );
        }
    }
}
