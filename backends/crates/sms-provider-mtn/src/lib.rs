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

/// `Provider.key` for this adapter — a deployment-market identity, not
/// MADAPI's own. MADAPI itself is pan-African (`host: api.mtn.com`, no
/// country segment anywhere in the vendored Swagger); this key names
/// *this deployment's* market, Cameroon (this repo's charter default —
/// see `AGENTS.md`), the same way `sms-provider-orange-cm`'s `"orange_cm"`
/// does. A second deployment targeting a different MTN opco through the
/// same MADAPI host would configure a second `Provider` row with a
/// different key, pointed at the same crate.
const KEY: &str = "mtn_cm";

/// The noun `classify_transport_error`'s `Indeterminate` message and
/// `classify_submit_error`'s `Unavailable`/`Permanent` messages all name
/// this provider as. Unlike `sms-provider-orange-cm`, which preserves a
/// historical case difference between its two provider-noun constants
/// verbatim from text that predates the `sms-provider-http` DRY-up, this
/// crate is a hard rewrite with no legacy text to preserve, so one
/// constant serves both roles.
const PROVIDER_NOUN: &str = "MTN";

/// MADAPI's own documented `429` behaviour for this endpoint is
/// undocumented — the Swagger's own response table for
/// `/messages/sms/outbound` lists only `401`/`404`/`407`/`500`, no
/// `429` at all. `sms_provider_http::classify_common_submit_status`
/// still handles a `429` defensively should MADAPI ever throttle this
/// way in practice (a reverse proxy in front of MADAPI could easily
/// introduce one MADAPI's own contract never promised), and this is the
/// delay used if it does — unchanged from this crate's own
/// pre-rewrite constant, chosen conservatively since no TPS ceiling is
/// documented to size it against.
const RATE_LIMIT_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

/// MADAPI's own canonical "success" `statusCode` — `resourceReference.statusCode`'s
/// own documented example (`mtn-sms-v3-swagger.yaml`,
/// `definitions.resourceReference.properties.statusCode`). Every other
/// 4-character value is an application-level failure reported through an
/// HTTP `200` — see [`classify_application_error`]'s own doc.
const MADAPI_SUCCESS_STATUS_CODE: &str = "0000";

/// `outboundSMSMessageRequest.clientCorrelatorId`'s own documented
/// `maxLength` (`mtn-sms-v3-swagger.yaml`,
/// `definitions.outboundSMSMessageRequest.properties.clientCorrelatorId`).
/// See [`MtnProvider::submit`]'s own doc and `lib.md`'s "`provider_ref_alt`
/// is genuinely used here" section for why this is checked rather than
/// silently truncated.
const MAX_CLIENT_CORRELATOR_ID_LEN: usize = 36;

/// What this adapter needs to talk to MADAPI, and the commercial terms of
/// this specific contract. Never a secret in the database (§2.4:
/// `Provider.credentialRef` is a pointer) — the worker resolves
/// `client_id`/`client_secret` at startup and constructs this, the same
/// convention `sms-provider-orange-cm::OrangeCmConfig` already
/// established.
#[derive(Clone)]
pub struct MtnConfig {
    /// The `OAuth2` `client_credentials` client id — see `token`'s own
    /// module doc for MADAPI's real token endpoint shape.
    pub client_id: String,
    /// The `OAuth2` `client_credentials` client secret. Never logged —
    /// see this struct's own hand-written [`std::fmt::Debug`] impl below.
    pub client_secret: String,
    /// The approved short code MADAPI's own `serviceCode` field requires
    /// unconditionally on every submit — see `lib.md`'s own
    /// "`serviceCode` is the fixed, required identity" section for why
    /// this is the config-level, always-sent identity, distinct from the
    /// optional, per-message `senderAddress` (see `sender_id` below).
    pub service_code: String,
    /// An operator-configured *default* alphanumeric sender, used only
    /// when a specific submission's own `SubmitRequest::sender_id` is
    /// empty. See [`MtnProvider::sender_address`] and `lib.md`'s own
    /// "`serviceCode` is the fixed, required identity" section for the
    /// full reasoning — in production this should never actually be
    /// needed, since `Message.senderIdValue` is schema-constrained to be
    /// non-empty. Deliberately excluded from the CLI's all-or-none
    /// required-credential group: MADAPI's own `senderAddress` is
    /// optional, so requiring an operator to set this would enforce a
    /// constraint MADAPI itself doesn't have.
    pub sender_id: Option<String>,
    /// MADAPI's API host, `https://api.mtn.com` in production (matching
    /// the vendored Swagger's own `host: api.mtn.com`). Overridable so
    /// tests can point this at a local mock server instead — `token_url`
    /// derives the token endpoint's own scheme+host+port from this same
    /// value rather than hardcoding `api.mtn.com` a second time, so a
    /// test pointing this at a mock server exercises both endpoints
    /// against it. See `token.md` for the full reasoning.
    pub base_url: String,
    /// The submission rate this specific MADAPI contract allows, in
    /// messages per second. Not published anywhere in the vendored
    /// Swagger — a negotiated commercial term, the same shape this
    /// crate's own original placeholder already established for
    /// `Capabilities` (see `lib.md`'s own "`Capabilities` stays
    /// contract-driven" section, unaffected by this rewrite).
    pub tps_ceiling: f64,
    /// What one segment costs on this contract, in XAF. `Decimal`, never
    /// a float — this is money, same discipline `sms-provider::Capabilities`
    /// itself requires.
    pub cost_per_segment_xaf: Decimal,
    /// Whether this specific MADAPI relationship has an alphanumeric
    /// sender registered and approved with MTN. `false` is the safer
    /// default for a `Default`-less config that must be constructed
    /// explicitly (this repo's own standing preference: no default that
    /// invents a fact — see `AGENTS.md` on `HashPepper`), because an
    /// unregistered alphanumeric sender submitted to MTN risks the
    /// message being silently rewritten or dropped, not a clean
    /// rejection this adapter could classify and route around.
    pub supports_alphanumeric_sender: bool,
    /// TCP/TLS connect timeout. Same testing rationale as
    /// `OrangeCmConfig::connect_timeout` — exposed as a knob so a live
    /// test can shrink it and prove the connect-vs-read-timeout
    /// distinction in `classify_transport_error` deterministically.
    pub connect_timeout: std::time::Duration,
    /// Overall request timeout — connect *and* send *and* await the
    /// response. Same rationale as `OrangeCmConfig::request_timeout`.
    pub request_timeout: std::time::Duration,
}

impl MtnConfig {
    /// A config for [`MtnProvider::subscribe_delivery_reports`] and
    /// nothing else.
    ///
    /// Registering a `deliveryReportUrl` neither sends a message nor
    /// prices one, so the three commercial terms
    /// (`tps_ceiling`/`cost_per_segment_xaf`/`supports_alphanumeric_sender`)
    /// are irrelevant on that path — `subscribe_delivery_reports` reads
    /// `client_id`, `client_secret`, `service_code` and `base_url`, and
    /// touches nothing else.
    ///
    /// This exists so the `sms-gateway mtn-subscribe-dlr` operator
    /// command does not have to invent numbers for them at its own call
    /// site, which is the thing this crate's `Default`-less config is
    /// specifically designed to prevent (`MtnConfig` has no `Default` so
    /// a real sending path can never silently acquire a made-up TPS
    /// ceiling or per-segment cost). The values below are placeholders
    /// and are documented as unread rather than as sensible defaults —
    /// **never** build a submitting provider from this.
    #[must_use]
    pub fn for_subscription_only(
        client_id: String,
        client_secret: String,
        service_code: String,
        base_url: String,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            service_code,
            sender_id: None,
            base_url,
            // Unread on the subscription path. Deliberately absurd rather
            // than plausible: if one of these ever does reach a submit or
            // a cost calculation, a zero TPS ceiling and a zero price are
            // far easier to spot in a log or an invoice than a
            // realistic-looking guess would be.
            tps_ceiling: 0.0,
            cost_per_segment_xaf: Decimal::ZERO,
            supports_alphanumeric_sender: false,
            connect_timeout: std::time::Duration::from_secs(10),
            request_timeout: std::time::Duration::from_secs(30),
        }
    }
}

impl std::fmt::Debug for MtnConfig {
    /// Hand-written, not derived — `client_secret` must never reach a
    /// `{:?}` log line. This crate's own previous shape derived `Debug`
    /// on its config type (which held an equally sensitive static
    /// `api_key`) with no redaction at all; that gap does not survive
    /// this rewrite. Same discipline as
    /// `sms-provider-orange-cm::OrangeCmConfig`'s own hand-written impl
    /// and `sms_api::pepper::HashPepper`'s.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtnConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("service_code", &self.service_code)
            .field("sender_id", &self.sender_id)
            .field("base_url", &self.base_url)
            .field("tps_ceiling", &self.tps_ceiling)
            .field("cost_per_segment_xaf", &self.cost_per_segment_xaf)
            .field(
                "supports_alphanumeric_sender",
                &self.supports_alphanumeric_sender,
            )
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

/// MTN via MADAPI: `OAuth2` token acquisition, SMS submission, DLR
/// parsing, and DLR-endpoint subscription. See the module doc for
/// exactly which parts of this are transcribed from the vendored
/// Swagger and which remain unverified against a live account.
pub struct MtnProvider {
    client: reqwest::Client,
    config: MtnConfig,
    token: token::TokenCache,
}

impl MtnProvider {
    /// Build an adapter from `config`. Cheap and synchronous, matching
    /// `OrangeCmProvider::new` — the first real network call happens on
    /// the first [`SmsProvider::submit`] or [`SmsProvider::health`], not
    /// here.
    ///
    /// # Panics
    ///
    /// Never in practice — the only way `ClientBuilder::build` fails is a
    /// TLS backend that can't initialise, and this crate's `rustls-tls`
    /// feature has no such failure mode with only timeouts configured
    /// (same reasoning as `OrangeCmProvider::new`'s own doc).
    #[must_use]
    pub fn new(config: MtnConfig) -> Self {
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

    /// A valid bearer token, fetching and caching a fresh one if the
    /// cached one has passed its 80%-of-lifetime refresh margin (`token`'s
    /// module doc).
    ///
    /// No in-flight gate on a refresh, for the identical reason
    /// `OrangeCmProvider::access_token`'s own doc gives: N concurrent
    /// `submit`/`health` calls that all observe an expired token each
    /// fetch and store their own — `TokenCache::store` makes the last
    /// write win, every fetched token is independently valid, and this
    /// is bounded by `capabilities().tps_ceiling` the same way Orange's
    /// own worst case is.
    async fn access_token(&self) -> Result<String, ProviderError> {
        if let Some(token) = self.token.valid() {
            return Ok(token);
        }

        let token_url = self.token_url()?;
        let fetched = token::fetch(
            &self.client,
            token_url,
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

    /// MADAPI's real token endpoint
    /// (`securityDefinitions.OAuth2.tokenUrl`,
    /// `mtn-sms-v3-swagger.yaml:19`) — an absolute URL on the same host
    /// as `base_url` but under `/v1/...`, not nested inside `base_url`'s
    /// own `/v3/sms/` `basePath`. See `token.md` for the full reasoning
    /// behind deriving this from `base_url`'s own scheme+host+port
    /// rather than hardcoding `api.mtn.com`, and for why
    /// `grant_type=client_credentials` belongs on this URL's query
    /// string rather than a form body.
    fn token_url(&self) -> Result<reqwest::Url, ProviderError> {
        let mut url = reqwest::Url::parse(&self.config.base_url).map_err(|error| {
            ProviderError::Unavailable {
                message: format!("invalid base_url: {error}"),
                source: Some(Box::new(error)),
            }
        })?;
        url.set_path("/v1/oauth/access_token/accesstoken");
        url.set_query(Some("grant_type=client_credentials"));
        Ok(url)
    }

    /// `POST {base_url}/v3/sms/messages/sms/outbound` —
    /// `mtn-sms-v3-swagger.yaml`'s own `basePath` (`/v3/sms/`) plus
    /// `paths./messages/sms/outbound`. Built via `Url::path_segments_mut`
    /// rather than string concatenation, so the encoding is the standard
    /// library's, matching `sms-provider-orange-cm::submit_url`'s own
    /// convention.
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
                // `()` on the `Err` side — no real error object exists.
                source: None,
            })?
            .extend(["v3", "sms", "messages", "sms", "outbound"]);
        Ok(url)
    }

    /// `POST {base_url}/v3/sms/messages/sms/subscription` —
    /// `mtn-sms-v3-swagger.yaml`'s own `paths./messages/sms/subscription`.
    /// See [`MtnProvider::subscribe_delivery_reports`].
    fn subscription_url(&self) -> Result<reqwest::Url, ProviderError> {
        let mut url = reqwest::Url::parse(&self.config.base_url).map_err(|error| {
            ProviderError::Unavailable {
                message: format!("invalid base_url: {error}"),
                source: Some(Box::new(error)),
            }
        })?;
        url.path_segments_mut()
            .map_err(|()| ProviderError::Unavailable {
                message: "base_url cannot be a base for path segments".to_owned(),
                source: None,
            })?
            .extend(["v3", "sms", "messages", "sms", "subscription"]);
        Ok(url)
    }

    /// The `senderAddress` to submit, if any — see `lib.md`'s own
    /// "`serviceCode` is the fixed, required identity" section for the
    /// full reasoning. Prefers the per-message `req.sender_id` (already
    /// resolved by `sendMessage` as the App-specific, approved sender for
    /// this exact message — the same field
    /// `sms-provider-orange-cm::submit` reads from for its own
    /// `senderName`), falling back to `self.config.sender_id` (an
    /// operator-configured default) only when the per-message value is
    /// empty. `None` when both are empty, which MADAPI's own Swagger
    /// treats as fully valid: `serviceCode` alone identifies the sender
    /// in that case.
    fn sender_address<'a>(&'a self, req: &'a SubmitRequest) -> Option<&'a str> {
        Some(req.sender_id.as_str())
            .filter(|value| !value.is_empty())
            .or(self.config.sender_id.as_deref())
            .filter(|value| !value.is_empty())
    }
}

/// `outboundSMSMessageRequest`
/// (`mtn-sms-v3-swagger.yaml`, `definitions.outboundSMSMessageRequest`) —
/// every field this crate sends, nothing more. `keyword` (the Swagger's
/// own Nigeria-opco-specific shared-short-code field) is deliberately
/// never sent: nothing in this deployment shares a short code with
/// another partner, and the Swagger itself scopes that field to "only
/// used and... requested by the Nigeria opco".
#[derive(Debug, Serialize)]
struct OutboundSmsMessageRequest<'a> {
    /// A JSON array, per the Swagger's own documented shape — MADAPI
    /// itself allows several recipients per request ("If more than one
    /// address is used the values will be comma separated" — read as:
    /// the array may hold more than one), but this adapter, matching
    /// `sms-provider-orange-cm`'s own one-message-per-call shape, always
    /// sends exactly one.
    #[serde(rename = "receiverAddress")]
    receiver_address: Vec<&'a str>,
    message: &'a str,
    #[serde(rename = "clientCorrelatorId")]
    client_correlator_id: &'a str,
    #[serde(rename = "serviceCode")]
    service_code: &'a str,
    /// Genuinely absent from the wire when `None`, never
    /// present-with-`null` — `skip_serializing_if` matches
    /// `sms-provider-orange-cm`'s own care about sending exactly the
    /// documented shape and no more.
    #[serde(rename = "senderAddress", skip_serializing_if = "Option::is_none")]
    sender_address: Option<&'a str>,
    /// Always `true`. MADAPI's own default is `false` ("By default this
    /// is set to false"), but this deployment always wants a delivery
    /// receipt when one is registered — the same posture Orange's own
    /// adapter takes implicitly by always requesting a DLR. The
    /// documented consequence, stated rather than silently assumed: "the
    /// consumer should ensure that they must have subscribed for
    /// delivery receipts... using the subscriptions endpoints" — sending
    /// `true` with no subscription registered is not an error MADAPI
    /// documents rejecting, it simply means no DLR arrives, which is the
    /// same "no DLR ever" failure mode `docs/architecture.md`'s own
    /// chaos-suite coverage already treats as an accepted, expiry-reaped
    /// outcome elsewhere in this codebase.
    #[serde(rename = "requestDeliveryReceipt")]
    request_delivery_receipt: bool,
}

/// `resourceReference` (`mtn-sms-v3-swagger.yaml`,
/// `definitions.resourceReference`) — only the fields this adapter
/// reads. `data.status` (MADAPI's own initial submission status, e.g.
/// `"PENDING"`) is deliberately not modelled: this adapter has no use
/// for it beyond what `dispatch`'s own state machine already tracks
/// generically the moment `submit` returns `Ok`, and serde ignores
/// unknown wire fields by default.
#[derive(Debug, Deserialize)]
struct ResourceReference {
    #[serde(rename = "statusCode")]
    status_code: String,
    #[serde(rename = "statusMessage")]
    status_message: String,
    #[serde(rename = "transactionId")]
    transaction_id: String,
}

/// `ShortCodeSubscription` (`mtn-sms-v3-swagger.yaml`,
/// `definitions.ShortCodeSubscription`) — see
/// [`MtnProvider::subscribe_delivery_reports`]'s own doc for why
/// `callback_url` and `delivery_report_url` are set to the same value.
#[derive(Debug, Serialize)]
struct ShortCodeSubscriptionRequest<'a> {
    #[serde(rename = "callbackUrl")]
    callback_url: &'a str,
    #[serde(rename = "targetSystem")]
    target_system: &'a str,
    #[serde(rename = "deliveryReportUrl", skip_serializing_if = "Option::is_none")]
    delivery_report_url: Option<&'a str>,
    #[serde(rename = "serviceCode")]
    service_code: &'a str,
}

/// `subscriptionResponse` (`mtn-sms-v3-swagger.yaml`,
/// `definitions.subscriptionResponse`) — only the fields this adapter
/// reads.
#[derive(Debug, Deserialize)]
struct SubscriptionResponse {
    #[serde(rename = "statusCode")]
    status_code: String,
    #[serde(rename = "statusMessage")]
    status_message: String,
    data: SubscriptionResponseData,
}

#[derive(Debug, Deserialize)]
struct SubscriptionResponseData {
    #[serde(rename = "subscriptionId")]
    subscription_id: Option<String>,
}

#[async_trait]
impl SmsProvider for MtnProvider {
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
        // Checked before any network call — a silently truncated
        // clientCorrelatorId would be sent, accepted, and produce a DLR
        // this system could never correlate back to a Message. See
        // `lib.md`'s own "`provider_ref_alt` is genuinely used here"
        // section.
        if req.reference.len() > MAX_CLIENT_CORRELATOR_ID_LEN {
            return Err(ProviderError::Permanent {
                code: "CLIENT_CORRELATOR_ID_TOO_LONG".to_owned(),
                message: format!(
                    "reference {:?} is {} bytes; MADAPI's own clientCorrelatorId maxLength is \
                     {MAX_CLIENT_CORRELATOR_ID_LEN}",
                    req.reference,
                    req.reference.len()
                ),
            });
        }

        let token = self.access_token().await?;
        let url = self.submit_url()?;

        let body = OutboundSmsMessageRequest {
            receiver_address: vec![&req.to],
            message: &req.body,
            client_correlator_id: &req.reference,
            service_code: &self.config.service_code,
            sender_address: self.sender_address(req),
            request_delivery_receipt: true,
        };

        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|error| classify_transport_error(error, PROVIDER_NOUN))?;

        let status = response.status();
        if status != StatusCode::OK {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_submit_error(status, &text));
        }

        // MADAPI answered 200 — but per lib.md's own "What a 200 on
        // /messages/sms/outbound actually means" section, that is only
        // "the request reached MADAPI," not "the message was accepted."
        // Any failure past this point (an unparseable body) means we
        // cannot even read MADAPI's own verdict, so `Indeterminate`
        // applies — the message may already be in flight and retrying
        // risks a duplicate send, the same reasoning
        // `sms-provider-orange-cm::submit` documents for its own
        // equivalent branch.
        let parsed: ResourceReference =
            response
                .json()
                .await
                .map_err(|error| ProviderError::Indeterminate {
                    message: format!(
                        "submit returned 200 but the response body was not valid JSON, so \
                         MADAPI's own statusCode could not be read: {error}"
                    ),
                    source: Some(Box::new(error)),
                })?;

        // Now MADAPI's own verdict IS readable — `statusCode != '0000'`
        // is not ambiguous the way an unparseable body is. MADAPI is
        // explicitly telling us this submission was not accepted, so
        // `Rejected`, not `Indeterminate`: see
        // `classify_application_error`'s own doc for the full reasoning.
        classify_application_error(&parsed.status_code, &parsed.status_message)?;

        if parsed.transaction_id.is_empty() {
            return Err(ProviderError::Indeterminate {
                message: "submit returned 200/0000 (accepted) but transactionId was empty"
                    .to_owned(),
                // The body parsed as valid JSON and MADAPI reported
                // success; it just lacked the field this adapter needs
                // to correlate a later DLR — no real error object to
                // chain.
                source: None,
            });
        }

        Ok(SubmitAck {
            provider_ref: parsed.transaction_id,
            // Unlike Orange, MADAPI's DLR echoes back this system's own
            // caller-supplied clientCorrelatorId — see lib.md's own
            // "`provider_ref_alt` is genuinely used here" section.
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

impl MtnProvider {
    /// Registers `delivery_report_url` against `self.config.service_code`
    /// with MADAPI, under `target_system` — the operationally real
    /// difference this rewrite unlocks versus Orange's manual-support-
    /// ticket DLR whitelisting. See `lib.md`'s own "`subscribe_delivery_reports`"
    /// section for the full framing.
    ///
    /// Nothing in this codebase calls this today. The intended caller is
    /// a future `sms-gateway` subcommand — the same operator-action
    /// shape `rotate-signing-key`/`provision-user` already use, not
    /// generated CRUD — that would wire an operator's own real
    /// `/dlr/{providerKey}` URL through here once, at provisioning time.
    ///
    /// `ShortCodeSubscription.callbackUrl` (Mobile-Originating messages)
    /// is set to the same value as `delivery_report_url` — a deliberate
    /// simplification, not an oversight: MADAPI's own schema requires
    /// `callbackUrl` unconditionally, even for a caller that only wants
    /// delivery receipts, and this codebase has no MO-message handling
    /// at all for a distinct URL to route into. If MTN ever POSTs a real
    /// inbound MO message to this URL, `dlr::parse` rejects it the same
    /// safe way it rejects any other body carrying no
    /// `clientCorrelatorId` — a `400`, not silent corruption. Revisit if
    /// MO handling is ever built.
    ///
    /// # Errors
    ///
    /// A transport failure, a non-`200`/non-`'0000'` MADAPI response
    /// (the same classification `submit`'s own `classify_submit_error`
    /// uses — a subscription attempt fails the identical shape of way a
    /// submission does), or a `200`/`'0000'` response carrying no
    /// `subscriptionId` in `data`.
    pub async fn subscribe_delivery_reports(
        &self,
        delivery_report_url: &str,
        target_system: &str,
    ) -> Result<String, ProviderError> {
        let token = self.access_token().await?;
        let url = self.subscription_url()?;

        let body = ShortCodeSubscriptionRequest {
            callback_url: delivery_report_url,
            target_system,
            delivery_report_url: Some(delivery_report_url),
            service_code: &self.config.service_code,
        };

        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|error| classify_transport_error(error, PROVIDER_NOUN))?;

        let status = response.status();
        if status != StatusCode::OK {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_submit_error(status, &text));
        }

        let parsed: SubscriptionResponse =
            response
                .json()
                .await
                .map_err(|error| ProviderError::Indeterminate {
                    message: format!(
                        "subscribe returned 200 but the response body was not valid JSON, so \
                         MADAPI's own statusCode could not be read: {error}"
                    ),
                    source: Some(Box::new(error)),
                })?;

        classify_application_error(&parsed.status_code, &parsed.status_message)?;

        parsed
            .data
            .subscription_id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| ProviderError::Indeterminate {
                message: "subscribe returned 200/0000 (accepted) but no subscriptionId was \
                          present in data"
                    .to_owned(),
                source: None,
            })
    }
}

/// `statusCode != '0000'` on an otherwise-successful HTTP response is
/// MADAPI's own way of reporting an application-level failure through a
/// transport-level `200` — see `lib.md`'s own "What a `200` on
/// `/messages/sms/outbound` actually means" section. `Ok(())` when the
/// status is `'0000'`; `Err(ProviderError::Rejected)` for anything else.
///
/// `Rejected`, not `Unavailable`/`Transient`/`Permanent`: nothing in the
/// vendored Swagger documents what any individual non-`'0000'` code
/// means (`statusCode`'s own doc points at "the MADAPI Confluence Page
/// 'Response Codes'" — not vendored here, and not accessible from this
/// crate), so there is no basis to guess whether a given code is
/// message-specific (favouring `Rejected`), provider-wide (favouring
/// `Unavailable`, which would trip the circuit breaker), or worth a
/// same-provider retry (favouring `Transient`). `Rejected` is the
/// narrowest, least-consequential-if-wrong of the four real variants: it
/// fails only this one message, opens no circuit breaker against a
/// provider that might be perfectly healthy, and does not loop forever
/// retrying a code that might be permanent — an operator reading
/// `Message.stateReason` (which carries `code`/`message` verbatim) can
/// diagnose and correct the specific case by hand. Revisit the moment a
/// real `statusCode` catalogue is available to classify against.
fn classify_application_error(
    status_code: &str,
    status_message: &str,
) -> Result<(), ProviderError> {
    if status_code == MADAPI_SUCCESS_STATUS_CODE {
        return Ok(());
    }
    Err(ProviderError::Rejected {
        code: format!("madapi_{status_code}"),
        message: status_message.to_owned(),
    })
}

/// Classifies a well-formed non-`200` response from `/messages/sms/outbound`
/// or `/messages/sms/subscription` — both endpoints share the identical
/// documented error-response set (`401`/`404`/`407`/`500`, each
/// `#/definitions/Error`), so one function serves both call sites.
///
/// `401` ("Not authenticated") — [`ProviderError::Permanent`]: nothing on
/// this provider can be attempted without valid credentials, but the
/// message itself may well be sendable elsewhere. Matches
/// `sms-provider-orange-cm::token::fetch`'s identical reasoning for a
/// bad-credentials response, applied here at submit time rather than
/// token time because MADAPI's own Swagger documents `401` as a possible
/// *submit* response, distinct from whatever the token endpoint itself
/// might return.
///
/// `407` ("Proxy system not authenticated") — [`ProviderError::Unavailable`]:
/// this is MADAPI's own *internal* proxy failing to authenticate to a
/// backend system, not a statement about this adapter's credentials —
/// nothing this adapter did is wrong, and the failure is exactly the
/// shape §6.3's circuit breaker exists for (this provider broadly
/// broken right now, try a different route, keep re-checking).
///
/// `404`/`500`/anything else falls to
/// `sms_provider_http::classify_common_submit_status`'s shared mapping
/// (`5xx` → `Unavailable`, everything else including `404` → `Rejected`
/// via its own `http_{status}` code) — undocumented for MADAPI beyond
/// the bare "Not found"/"Internal Server Error" text, and nothing about
/// either status is MADAPI-specific enough to warrant its own arm here.
fn classify_submit_error(status: StatusCode, body: &str) -> ProviderError {
    if status == StatusCode::UNAUTHORIZED {
        return ProviderError::Permanent {
            code: format!("auth_{}", status.as_u16()),
            message: format!("MTN rejected the request as unauthenticated: {body}"),
        };
    }
    if status == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        return ProviderError::Unavailable {
            message: format!("MTN's own proxy system reported it is not authenticated: {body}"),
            // A real response was received and read as text — no
            // leftover `reqwest::Error` to chain.
            source: None,
        };
    }
    classify_common_submit_status(status, body, PROVIDER_NOUN, RATE_LIMIT_RETRY_AFTER)
}

#[cfg(test)]
mod tests {
    use super::{
        KEY, MADAPI_SUCCESS_STATUS_CODE, MAX_CLIENT_CORRELATOR_ID_LEN, MtnConfig, MtnProvider,
        PROVIDER_NOUN, classify_transport_error,
    };
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

    fn config(base_url: String) -> MtnConfig {
        MtnConfig {
            client_id: "test-client-id".to_owned(),
            client_secret: "test-client-secret".to_owned(),
            service_code: "131".to_owned(),
            sender_id: None,
            base_url,
            tps_ceiling: 20.0,
            cost_per_segment_xaf: rust_decimal::Decimal::new(15, 0),
            supports_alphanumeric_sender: true,
            connect_timeout: TEST_CONNECT_TIMEOUT,
            request_timeout: TEST_REQUEST_TIMEOUT,
        }
    }

    fn provider(base_url: String) -> MtnProvider {
        MtnProvider::new(config(base_url))
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

    /// The real token endpoint (`token.md`'s own doc: an absolute URL,
    /// host derived from `base_url`, `grant_type` on the query string,
    /// no request body). Mounted with no matcher on the query string
    /// itself — wiremock's `path()` matcher ignores it — since asserting
    /// the exact query is `submit_sends_grant_type_only_on_the_query_string`'s
    /// own job below, not every test's.
    async fn mock_token_endpoint(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/v1/oauth/access_token/accesstoken"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "test-token",
                "expires_in": 3600,
            })))
            .mount(server)
            .await;
    }

    fn ok_resource_reference(transaction_id: &str) -> serde_json::Value {
        serde_json::json!({
            "statusCode": MADAPI_SUCCESS_STATUS_CODE,
            "statusMessage": "Sucessful",
            "transactionId": transaction_id,
            "data": {"status": "PENDING"},
        })
    }

    #[test]
    fn key_is_the_providers_schema_key() {
        assert_eq!(provider(String::new()).key(), KEY);
    }

    #[test]
    fn capabilities_read_config_rather_than_a_compiled_in_constant() {
        let provider = MtnProvider::new(MtnConfig {
            tps_ceiling: 42.5,
            cost_per_segment_xaf: rust_decimal::Decimal::new(21, 0),
            supports_alphanumeric_sender: false,
            ..config("https://example.invalid".to_owned())
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

    #[test]
    fn debug_redacts_the_client_secret() {
        let cfg = MtnConfig {
            client_secret: "NEVER-PRINT-THIS".to_owned(),
            ..config("https://example.invalid".to_owned())
        };
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains("NEVER-PRINT-THIS"),
            "client_secret reached a Debug rendering: {rendered}"
        );
        assert!(rendered.contains("<redacted>"), "{rendered}");
        assert!(rendered.contains("test-client-id"), "{rendered}");
    }

    #[tokio::test]
    async fn submit_succeeds_and_extracts_the_transaction_id() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_resource_reference("txn-42")))
            .mount(&server)
            .await;

        let ack = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap();

        assert_eq!(ack.provider_ref, "txn-42");
        assert_eq!(
            ack.provider_ref_alt,
            Some("msg-1".to_owned()),
            "MADAPI's DLR echoes back clientCorrelatorId — provider_ref_alt must carry it, \
             unlike Orange's own always-None provider_ref_alt; see lib.md"
        );
    }

    #[tokio::test]
    async fn submit_sends_service_code_and_the_configured_receiver() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "serviceCode": "131",
                "receiverAddress": ["+237677123456"],
                "clientCorrelatorId": "msg-9",
                "requestDeliveryReceipt": true,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_resource_reference("txn-9")))
            .mount(&server)
            .await;

        provider(server.uri())
            .submit(&request("msg-9"))
            .await
            .unwrap();
    }

    /// `senderAddress` comes from `req.sender_id` (the per-message,
    /// already-approved sender), not a fixed config value — see
    /// `sender_address`'s own doc. Proven at the wire level, matching
    /// this repo's own house standard for a claim about what actually
    /// gets sent.
    #[tokio::test]
    async fn submit_sends_the_per_message_sender_id_as_sender_address() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "senderAddress": "VYMALO",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_resource_reference("txn-10")))
            .mount(&server)
            .await;

        provider(server.uri())
            .submit(&request("msg-10"))
            .await
            .unwrap();
    }

    /// When neither the per-message `sender_id` nor the config-level
    /// fallback carries a value, `senderAddress` must be genuinely
    /// absent from the wire, not sent as an empty string or `null` — the
    /// same "absent, not present-with-nulls" discipline Orange's own
    /// `receiptRequest` test enforces.
    #[tokio::test]
    async fn submit_omits_sender_address_entirely_when_none_is_available() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_resource_reference("txn-11")))
            .mount(&server)
            .await;

        let mut req = request("msg-11");
        req.sender_id = String::new();
        provider(server.uri()).submit(&req).await.unwrap();

        let requests = server.received_requests().await.expect("recording enabled");
        let submit_request = requests
            .iter()
            .find(|r| r.url.path().ends_with("/outbound"))
            .expect("the submit request was recorded");
        let body: serde_json::Value = submit_request.body_json().unwrap();
        assert!(
            body.get("senderAddress").is_none(),
            "senderAddress must be genuinely absent, not present-with-nulls: {body}"
        );
    }

    /// The config-level `sender_id` fallback, proven independently of the
    /// per-message value.
    #[tokio::test]
    async fn submit_falls_back_to_the_configured_default_sender_id() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "senderAddress": "DEFAULT-SENDER",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_resource_reference("txn-12")))
            .mount(&server)
            .await;

        let cfg = MtnConfig {
            sender_id: Some("DEFAULT-SENDER".to_owned()),
            ..config(server.uri())
        };
        let mut req = request("msg-12");
        req.sender_id = String::new();
        MtnProvider::new(cfg).submit(&req).await.unwrap();
    }

    /// **Guard-failure proof (b)**: a `200` HTTP response carrying a
    /// non-`'0000'` `statusCode` must not be treated as success. Proven
    /// directly against `classify_application_error` and, separately,
    /// through `submit` end to end.
    #[test]
    fn a_non_success_status_code_is_rejected_even_though_it_was_200() {
        let error = super::classify_application_error("1000", "sender id not approved")
            .expect_err("a non-0000 statusCode must not be Ok");
        match error {
            ProviderError::Rejected { code, message } => {
                assert_eq!(code, "madapi_1000");
                assert_eq!(message, "sender id not approved");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_treats_a_200_with_a_non_zero_status_code_as_rejected_not_success() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "statusCode": "1000",
                "statusMessage": "sender id not approved",
                "transactionId": "txn-13",
                "data": {"status": "REJECTED"},
            })))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-13"))
            .await
            .unwrap_err();

        assert!(
            matches!(error, ProviderError::Rejected { .. }),
            "expected Rejected, got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_rate_limited_submit_is_transient() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
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
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Unavailable { .. }));
    }

    #[tokio::test]
    async fn a_401_at_submit_time_is_permanent() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(401).set_body_string("not authenticated"))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Permanent { .. }));
    }

    #[tokio::test]
    async fn a_407_proxy_auth_failure_is_unavailable_not_permanent() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(
                ResponseTemplate::new(407).set_body_string("proxy system not authenticated"),
            )
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(
            matches!(error, ProviderError::Unavailable { .. }),
            "MTN's own proxy failing to authenticate to a backend is not this adapter's \
             credentials being bad: expected Unavailable, got {error:?}"
        );
    }

    #[tokio::test]
    async fn an_undocumented_404_is_rejected_via_the_shared_default() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .submit(&request("msg-1"))
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Rejected { .. }));
    }

    /// A `2xx` response is MADAPI telling us the submission reached it —
    /// unparseable body or not. Treating this as `Unavailable` (safe to
    /// retry) would resubmit an SMS MADAPI may already have accepted.
    /// Mirrors `sms-provider-orange-cm`'s identical test.
    #[tokio::test]
    async fn submit_returns_200_but_an_unparseable_body_is_indeterminate() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
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
    async fn submit_returns_200_and_0000_but_an_empty_transaction_id_is_indeterminate() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "statusCode": MADAPI_SUCCESS_STATUS_CODE,
                "statusMessage": "Sucessful",
                "transactionId": "",
                "data": {"status": "PENDING"},
            })))
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

    /// **Guard-failure proof (c)**: `clientCorrelatorId` past MADAPI's
    /// own documented 36-character `maxLength` must be refused before
    /// any network call, never silently truncated.
    #[tokio::test]
    async fn a_reference_past_the_client_correlator_id_length_limit_is_refused() {
        let too_long = "x".repeat(MAX_CLIENT_CORRELATOR_ID_LEN + 1);
        assert_eq!(too_long.len(), MAX_CLIENT_CORRELATOR_ID_LEN + 1);

        // No server mounted at all — this must fail before any request
        // is attempted, proven by using a base_url nothing listens on.
        let error = provider("http://127.0.0.1:1".to_owned())
            .submit(&request(&too_long))
            .await
            .unwrap_err();

        assert!(
            matches!(error, ProviderError::Permanent { .. }),
            "expected Permanent, got {error:?}"
        );
    }

    /// A reference exactly at the limit, and one comfortably under it
    /// (matching a real `Message.id`'s 23-character `Cuid` shape, per
    /// `AGENTS.md`), must both be accepted — this guard must not be
    /// off-by-one.
    #[tokio::test]
    async fn a_reference_at_or_under_the_length_limit_is_accepted() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_resource_reference("txn-ok")))
            .mount(&server)
            .await;

        let exactly_at_limit = "x".repeat(MAX_CLIENT_CORRELATOR_ID_LEN);
        provider(server.uri())
            .submit(&request(&exactly_at_limit))
            .await
            .expect("exactly at the limit must be accepted, not off-by-one refused");

        let real_shape_cuid = "c".repeat(23);
        provider(server.uri())
            .submit(&request(&real_shape_cuid))
            .await
            .expect("a real Message.id's own 23-character length must never trip this guard");
    }

    #[tokio::test]
    async fn health_is_ok_when_a_token_can_be_fetched() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;

        assert!(provider(server.uri()).health().await.healthy);
    }

    #[tokio::test]
    async fn health_is_unhealthy_when_the_token_endpoint_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/access_token/accesstoken"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let health = provider(server.uri()).health().await;
        assert!(!health.healthy);
        assert!(health.detail.is_some());
    }

    /// `token.md`'s own doc claims `grant_type=client_credentials` is
    /// sent once, on the token URL's query string, never a second time
    /// in a form-encoded body. Proven at the wire level.
    #[tokio::test]
    async fn the_token_request_carries_grant_type_only_on_the_query_string_never_in_the_body() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/outbound"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_resource_reference("txn-14")))
            .mount(&server)
            .await;

        provider(server.uri())
            .submit(&request("msg-14"))
            .await
            .unwrap();

        let requests = server.received_requests().await.expect("recording enabled");
        let token_request = requests
            .iter()
            .find(|r| r.url.path() == "/v1/oauth/access_token/accesstoken")
            .expect("the token request was recorded");
        assert_eq!(
            token_request.url.query(),
            Some("grant_type=client_credentials"),
            "grant_type must be on the query string"
        );
        assert!(
            token_request.body.is_empty(),
            "no second, form-encoded grant_type in the body: {:?}",
            String::from_utf8_lossy(&token_request.body)
        );
    }

    /// Proves `sms_provider_http::classify_transport_error`'s safe branch
    /// *through this adapter's own call site* — with a real
    /// connection-refused error rather than a mocked one. Identical setup
    /// to `sms-provider-orange-cm::a_connect_refusal_is_still_unavailable`.
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
            classify_transport_error(error, PROVIDER_NOUN),
            ProviderError::Unavailable { .. }
        ));
    }

    /// Proves `sms_provider_http::classify_transport_error`'s unsafe
    /// branch *through this adapter's own call site*.
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
        let classified = classify_transport_error(error, PROVIDER_NOUN);
        assert!(
            matches!(classified, ProviderError::Indeterminate { .. }),
            "expected Indeterminate, got {classified:?}"
        );
    }

    #[tokio::test]
    async fn subscribe_delivery_reports_returns_the_subscription_id() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/subscription"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "callbackUrl": "https://gateway.example/dlr/mtn_cm",
                "deliveryReportUrl": "https://gateway.example/dlr/mtn_cm",
                "targetSystem": "vsms",
                "serviceCode": "131",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "statusCode": MADAPI_SUCCESS_STATUS_CODE,
                "statusMessage": "Sms Subscriptions successful",
                "transactionId": "txn-sub-1",
                "data": {"subscriptionId": "sub-123"},
            })))
            .mount(&server)
            .await;

        let subscription_id = provider(server.uri())
            .subscribe_delivery_reports("https://gateway.example/dlr/mtn_cm", "vsms")
            .await
            .unwrap();

        assert_eq!(subscription_id, "sub-123");
    }

    #[tokio::test]
    async fn subscribe_delivery_reports_rejects_a_non_success_status_code() {
        let server = MockServer::start().await;
        mock_token_endpoint(&server).await;
        Mock::given(method("POST"))
            .and(path("/v3/sms/messages/sms/subscription"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "statusCode": "4004",
                "statusMessage": "Service code has already been registered.",
                "transactionId": "txn-sub-2",
                "data": {},
            })))
            .mount(&server)
            .await;

        let error = provider(server.uri())
            .subscribe_delivery_reports("https://gateway.example/dlr/mtn_cm", "vsms")
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Rejected { .. }));
    }
}
