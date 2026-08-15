#![doc = include_str!("lib.md")]

mod dlr;

use async_trait::async_trait;
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sms_provider::{
    Capabilities, DeliveryUpdate, Health, ProviderError, RawCallback, SmsProvider, SubmitAck,
    SubmitRequest,
};

const KEY: &str = "mtn_aggregator";

/// What this adapter needs to talk to the aggregator, and the commercial
/// terms of that specific contract. Never a secret in the database (§2.4:
/// `Provider.credentialRef` is a pointer) — the worker resolves `api_key` at
/// startup and constructs this, the same convention
/// `sms-provider-orange-cm::OrangeCmConfig` already established.
#[derive(Debug, Clone)]
pub struct MtnAggregatorConfig {
    /// The aggregator-issued API key, sent as `Authorization: Bearer
    /// <api_key>` on every request. See the module doc for why this crate
    /// assumes a static key rather than `OAuth2 client_credentials`.
    pub api_key: String,
    /// The approved sender ID (numeric, alphanumeric, or short code) to
    /// submit under. Whether an alphanumeric value here is actually usable
    /// is exactly what `supports_alphanumeric_sender` below declares.
    pub sender_id: String,
    /// The aggregator's API host, e.g. `https://api.example-aggregator.com`.
    /// Overridable so tests can point this at a local mock server instead
    /// of a real endpoint — no real aggregator host is known at the time
    /// this crate was written, so unlike `OrangeCmConfig::production()`
    /// there is no equivalent convenience constructor baking one in.
    pub base_url: String,
    /// The submission rate this specific aggregator contract allows, in
    /// messages per second. Unlike Orange's hard, published, self-service 5
    /// TPS ceiling, this is a negotiated commercial term with no public
    /// number to hardcode — see the module doc's `Capabilities` section.
    pub tps_ceiling: f64,
    /// What one segment costs on this contract, in XAF. `Decimal`, never a
    /// float — this is money, same discipline `sms-provider::Capabilities`
    /// itself requires.
    pub cost_per_segment_xaf: Decimal,
    /// Whether this specific aggregator relationship has an alphanumeric
    /// sender ID registered and approved with MTN. `false` is the safer
    /// default for a `Default`-less config that must be constructed
    /// explicitly (this repo's own standing preference: no default that
    /// invents a fact — see `AGENTS.md` on `HashPepper`), because an
    /// unregistered alphanumeric sender submitted to MTN risks the message
    /// being silently rewritten to a generic numeric string or dropped by
    /// MTN's firewall (§6.4's own "grey route" symptom list), not a clean
    /// rejection this adapter could classify and route around.
    pub supports_alphanumeric_sender: bool,
    /// TCP/TLS connect timeout. Same testing rationale as
    /// `OrangeCmConfig::connect_timeout` — exposed as a knob so a live test
    /// can shrink it and prove the connect-vs-read-timeout distinction in
    /// `classify_transport_error` deterministically.
    pub connect_timeout: std::time::Duration,
    /// Overall request timeout — connect *and* send *and* await the
    /// response. Same rationale as `OrangeCmConfig::request_timeout`.
    pub request_timeout: std::time::Duration,
}

/// MTN Cameroon via a licensed aggregator: API-key auth, SMS submission, DLR
/// parsing. See the module doc for exactly which parts of this are verified
/// reasoning and which are an invented, provisional shape.
pub struct MtnAggregatorProvider {
    client: reqwest::Client,
    config: MtnAggregatorConfig,
}

impl MtnAggregatorProvider {
    /// Build an adapter from `config`. Cheap and synchronous, matching
    /// `OrangeCmProvider::new` — the first real network call happens on the
    /// first [`SmsProvider::submit`] or [`SmsProvider::health`], not here.
    ///
    /// # Panics
    ///
    /// Never in practice — the only way `ClientBuilder::build` fails is a
    /// TLS backend that can't initialise, and this crate's `rustls-tls`
    /// feature has no such failure mode with only timeouts configured (same
    /// reasoning as `OrangeCmProvider::new`'s own doc).
    #[must_use]
    pub fn new(config: MtnAggregatorConfig) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(config.connect_timeout)
                .timeout(config.request_timeout)
                .build()
                .expect("reqwest client builder with only timeouts set never fails"),
            config,
        }
    }

    /// `POST {base_url}/v1/messages` — see the module doc for why this path
    /// is an invented placeholder, not a transcribed spec.
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
            .extend(["v1", "messages"]);
        Ok(url)
    }

    /// `GET {base_url}/v1/account` — same provisional-shape caveat as
    /// `submit_url`. No real aggregator publishes this path; it stands in
    /// for whatever "prove the API key still works without spending
    /// message quota" endpoint a real contract would document. Orange's
    /// `health` reuses its own token fetch for exactly this purpose
    /// (`access_token` as a side-effect-free credential check); this
    /// adapter has no token endpoint to reuse, so it needs an explicit one.
    fn account_url(&self) -> Result<reqwest::Url, ProviderError> {
        let mut url = reqwest::Url::parse(&self.config.base_url).map_err(|error| {
            ProviderError::Unavailable {
                message: format!("invalid base_url: {error}"),
            }
        })?;
        url.path_segments_mut()
            .map_err(|()| ProviderError::Unavailable {
                message: "base_url cannot be a base for path segments".to_owned(),
            })?
            .extend(["v1", "account"]);
        Ok(url)
    }
}

#[derive(Debug, Serialize)]
struct SubmitBody<'a> {
    to: &'a str,
    from: &'a str,
    text: &'a str,
    reference: &'a str,
}

#[derive(Debug, Deserialize)]
struct SubmitResponse {
    #[serde(rename = "messageId")]
    message_id: String,
}

#[async_trait]
impl SmsProvider for MtnAggregatorProvider {
    fn key(&self) -> &str {
        KEY
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            dlr: true,
            alphanumeric_sender: self.config.supports_alphanumeric_sender,
            ucs2: true,
            concatenation: true,
            tps_ceiling: self.config.tps_ceiling,
            cost_per_segment_xaf: self.config.cost_per_segment_xaf,
        }
    }

    async fn submit(&self, req: &SubmitRequest) -> Result<SubmitAck, ProviderError> {
        let url = self.submit_url()?;

        let body = SubmitBody {
            to: &req.to,
            from: &self.config.sender_id,
            text: &req.body,
            reference: &req.reference,
        };

        let response = self
            .client
            .post(url)
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| classify_transport_error(&error))?;

        let status = response.status();
        if status != StatusCode::CREATED {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_submit_error(status, &text));
        }

        // From here down the aggregator already answered 201 — the
        // submission was accepted. Any failure past this point (an
        // unparseable body, an empty messageId) means we cannot recover the
        // provider_ref to store, but the message may already be in flight,
        // so `Indeterminate` applies, not `Unavailable` — the same
        // reasoning `sms-provider-orange-cm::submit` documents for its own
        // equivalent branch.
        let parsed: SubmitResponse =
            response
                .json()
                .await
                .map_err(|error| ProviderError::Indeterminate {
                    message: format!(
                        "submit returned 201 (accepted) but the response body was not valid \
                         JSON, so no provider_ref could be recovered: {error}"
                    ),
                })?;

        if parsed.message_id.is_empty() {
            return Err(ProviderError::Indeterminate {
                message: "submit returned 201 (accepted) but messageId was empty".to_owned(),
            });
        }

        Ok(SubmitAck {
            provider_ref: parsed.message_id,
            // Unlike Orange, this crate's assumed DLR shape echoes back the
            // same messageId submit() already returns (see the module doc's
            // `Capabilities` section) — there is no second reference form
            // to carry, so `provider_ref_alt` stays `None`.
            provider_ref_alt: None,
        })
    }

    fn parse_dlr(&self, raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
        dlr::parse(raw)
    }

    async fn health(&self) -> Health {
        let url = match self.account_url() {
            Ok(url) => url,
            Err(error) => return Health::unhealthy(error.to_string()),
        };
        match self
            .client
            .get(url)
            .bearer_auth(&self.config.api_key)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => Health::ok(),
            Ok(response) => {
                Health::unhealthy(format!("account endpoint returned {}", response.status()))
            }
            Err(error) => Health::unhealthy(format!("account endpoint unreachable: {error}")),
        }
    }
}

/// Classifies a *transport*-level failure from `.send()` itself — the
/// network call never produced an HTTP response at all, as distinct from a
/// well-formed response carrying a non-`201` status
/// ([`classify_submit_error`]'s job).
///
/// Identical reasoning to `sms-provider-orange-cm::classify_transport_error`
/// — this logic is provider-agnostic (it follows from what `reqwest`
/// guarantees about `is_connect`/`is_timeout`/`is_body`, not from any
/// aggregator's documentation), so it is exactly as trustworthy here as
/// there, unlike the invented request/response shape elsewhere in this
/// file. **The predicate is connect-vs-read, not "is it a timeout":**
///
/// - [`reqwest::Error::is_connect`] is checked first and returns early —
///   `true` only for a failure establishing the connection itself (DNS,
///   TCP handshake, TLS handshake, including a connect-phase timeout). Not
///   a single byte of the request has reached the aggregator at that point,
///   so retrying — this provider or the next — is exactly as safe as it
///   always was: [`ProviderError::Unavailable`].
/// - Only past that check does [`reqwest::Error::is_timeout`] (which is
///   `true` for a timeout at *either* phase) or [`reqwest::Error::is_body`]
///   (a reset/closed connection mid-transfer) mean the connection was
///   already established when the failure hit — the aggregator may already
///   have the full request and be acting on it. Genuinely unknown, not
///   safe to retry: [`ProviderError::Indeterminate`].
/// - Anything else (a `Builder`/`Request`-kind error, a bad `Url`, a
///   redirect-policy violation) never got as far as writing to a socket,
///   so it stays [`ProviderError::Unavailable`].
///
/// Getting this backwards either destroys legitimate failover or
/// reintroduces the duplicate-SMS bug #119 exists to prevent.
fn classify_transport_error(error: &reqwest::Error) -> ProviderError {
    if error.is_connect() {
        return ProviderError::Unavailable {
            message: format!("submit request failed to connect: {error}"),
        };
    }
    if error.is_timeout() || error.is_body() {
        return ProviderError::Indeterminate {
            message: format!(
                "submit request timed out or was interrupted after the connection was \
                 already established; the aggregator may have received it: {error}"
            ),
        };
    }
    ProviderError::Unavailable {
        message: format!("submit request failed: {error}"),
    }
}

/// Classifies a well-formed non-`201` response. Unlike
/// `sms-provider-orange-cm::classify_submit_error`, which can cite §6.2's
/// specific "429 is the 5 TPS ceiling" claim, this crate has no real
/// aggregator documentation to draw the same distinctions from — see the
/// module doc. The mapping below is a conservative, defensible default
/// given only HTTP status codes, not a transcribed spec:
///
/// - `401`/`403` — the API key itself is bad. Nothing on this provider can
///   be attempted without one, but the message itself may well be sendable
///   elsewhere, so [`ProviderError::Permanent`] (try a different provider),
///   matching `sms-provider-orange-cm::token::fetch`'s identical reasoning
///   for a bad-credentials response at Orange's token endpoint.
/// - `429` — rate limited. [`ProviderError::Transient`]; retry this same
///   provider, never fail over. A generic aggregator API is more likely
///   than Orange's own bespoke endpoint to send a standard `Retry-After`
///   header (RFC 9110 §10.2.3), so it's parsed opportunistically — an
///   integer-seconds value if present, a conservative 5s default
///   otherwise (longer than Orange's 1s default: this crate has no
///   documented TPS ceiling to size the wait against, so it errs
///   conservative rather than hammering an aggregator that just rate
///   limited it).
/// - `5xx` — the aggregator's own backend. [`ProviderError::Unavailable`]:
///   fail over and count toward the circuit breaker.
/// - Everything else — conservatively [`ProviderError::Rejected`], the
///   safer of the two remaining options (it fails this one message rather
///   than risking a failover storm across every provider for what might be
///   a systematically bad sender ID), same default
///   `sms-provider-orange-cm::classify_submit_error` chose for the
///   identical reason.
fn classify_submit_error(status: StatusCode, body: &str) -> ProviderError {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return ProviderError::Permanent {
            code: format!("auth_{}", status.as_u16()),
            message: format!("aggregator rejected the api key: {body}"),
        };
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return ProviderError::Transient {
            retry_after: std::time::Duration::from_secs(5),
            message: format!("rate limited: {body}"),
        };
    }
    if status.is_server_error() {
        return ProviderError::Unavailable {
            message: format!("aggregator returned {status}: {body}"),
        };
    }
    ProviderError::Rejected {
        code: format!("http_{}", status.as_u16()),
        message: body.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_transport_error, MtnAggregatorConfig, MtnAggregatorProvider, KEY};
    use sms_encoding::SmsEncoding;
    use sms_provider::{ProviderError, SmsProvider, SubmitRequest};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Generous enough that no existing test (all fast, local wiremock
    /// responses) ever brushes against it, short enough that the
    /// deliberately-slow indeterminate-timeout tests below don't need to
    /// wait long. Same convention as `sms-provider-orange-cm`'s own test
    /// timeouts.
    const TEST_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    const TEST_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    fn provider(base_url: String) -> MtnAggregatorProvider {
        MtnAggregatorProvider::new(MtnAggregatorConfig {
            api_key: "test-key".to_owned(),
            sender_id: "VYMALO".to_owned(),
            base_url,
            tps_ceiling: 20.0,
            cost_per_segment_xaf: rust_decimal::Decimal::new(15, 0),
            supports_alphanumeric_sender: true,
            connect_timeout: TEST_CONNECT_TIMEOUT,
            request_timeout: TEST_REQUEST_TIMEOUT,
        })
    }

    fn request(reference: &str) -> SubmitRequest {
        SubmitRequest {
            to: "+237677123456".to_owned(),
            sender_id: "VYMALO".to_owned(),
            body: "Votre code est 4821".to_owned(),
            encoding: SmsEncoding::Gsm7,
            reference: reference.to_owned(),
        }
    }

    #[test]
    fn key_is_the_providers_schema_key() {
        assert_eq!(provider(String::new()).key(), KEY);
    }

    #[test]
    fn capabilities_read_config_rather_than_a_compiled_in_constant() {
        let provider = MtnAggregatorProvider::new(MtnAggregatorConfig {
            api_key: "k".to_owned(),
            sender_id: "S".to_owned(),
            base_url: "https://example.invalid".to_owned(),
            tps_ceiling: 42.5,
            cost_per_segment_xaf: rust_decimal::Decimal::new(21, 0),
            supports_alphanumeric_sender: false,
            connect_timeout: TEST_CONNECT_TIMEOUT,
            request_timeout: TEST_REQUEST_TIMEOUT,
        });
        let capabilities = provider.capabilities();
        assert!(
            (capabilities.tps_ceiling - 42.5).abs() < f64::EPSILON,
            "tps_ceiling must come from config, not a hardcoded constant like Orange's"
        );
        assert_eq!(
            capabilities.cost_per_segment_xaf,
            rust_decimal::Decimal::new(21, 0)
        );
        assert!(
            !capabilities.alphanumeric_sender,
            "an unregistered alphanumeric sender must not be reported as usable"
        );
    }

    #[tokio::test]
    async fn submit_succeeds_and_extracts_the_message_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "messageId": "mtn-res-42",
                "status": "Sent"
            })))
            .mount(&server)
            .await;

        let ack = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap();

        assert_eq!(ack.provider_ref, "mtn-res-42");
        assert_eq!(
            ack.provider_ref_alt, None,
            "this crate's assumed DLR shape correlates on the same messageId submit() \
             returns, unlike Orange's callbackData workaround — see the module doc"
        );
    }

    #[tokio::test]
    async fn submit_sends_the_configured_sender_id_as_from() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "from": "VYMALO",
                "reference": "msg-9"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "messageId": "mtn-res-9"
            })))
            .mount(&server)
            .await;

        provider(server.uri())
            .submit(&request("msg-9"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_rate_limited_submit_is_transient() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Transient { .. }));
    }

    #[tokio::test]
    async fn a_server_error_submit_is_unavailable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Unavailable { .. }));
    }

    #[tokio::test]
    async fn a_bad_api_key_is_permanent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_string("invalid api key"))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Permanent { .. }));
    }

    #[tokio::test]
    async fn an_unrecognised_client_error_is_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(400).set_body_string("malformed destination"))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Rejected { .. }));
    }

    /// A 2xx (`201`) response is the aggregator telling us the submission
    /// was accepted — unparseable body or not. Treating this as
    /// `Unavailable` (safe to retry) would resubmit an SMS the aggregator
    /// has already agreed to send. Mirrors
    /// `sms-provider-orange-cm`'s identical test.
    #[tokio::test]
    async fn submit_returns_201_but_an_unparseable_body_is_indeterminate() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(201).set_body_string("not json"))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(
            matches!(error, ProviderError::Indeterminate { .. }),
            "expected Indeterminate, got {error:?}"
        );
    }

    /// Same reasoning, for the narrower failure of a well-formed `201` body
    /// that carries an empty `messageId` — still a `201`, still an accepted
    /// submission.
    #[tokio::test]
    async fn submit_returns_201_but_an_empty_message_id_is_indeterminate() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(serde_json::json!({"messageId": ""})),
            )
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(
            matches!(error, ProviderError::Indeterminate { .. }),
            "expected Indeterminate, got {error:?}"
        );
    }

    #[tokio::test]
    async fn health_is_ok_when_the_account_endpoint_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/account"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        assert!(provider(server.uri()).health().await.healthy);
    }

    #[tokio::test]
    async fn health_is_unhealthy_when_the_account_endpoint_fails() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/account"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let health = provider(server.uri()).health().await;
        assert!(!health.healthy);
        assert!(health.detail.is_some());
    }

    /// Proves `classify_transport_error`'s safe branch directly, with a
    /// real connection-refused error rather than a mocked one — identical
    /// setup to `sms-provider-orange-cm::a_connect_refusal_is_still_unavailable`.
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
            classify_transport_error(&error),
            ProviderError::Unavailable { .. }
        ));
    }

    /// Proves `classify_transport_error`'s unsafe branch directly: a
    /// connection that *does* establish, against a server that then never
    /// answers before the client's own timeout fires — the shape a
    /// slow/hung aggregator endpoint produces, where the request may
    /// already be sitting on the aggregator's side. Identical setup to
    /// `sms-provider-orange-cm::a_post_connect_timeout_is_indeterminate` —
    /// this is provider-agnostic `reqwest` behaviour, proven the same way
    /// in both crates on purpose.
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
        assert!(matches!(
            classify_transport_error(&error),
            ProviderError::Indeterminate { .. }
        ));
    }
}
