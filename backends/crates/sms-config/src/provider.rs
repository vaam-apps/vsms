#![doc = include_str!("provider.md")]

use std::time::Duration;

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::error::ConfigError;
use crate::secret::Secret;

/// `Provider.key` of Orange Cameroon, and its `code` in `providers`.
pub const ORANGE_CM: &str = "orange_cm";
/// `Provider.key` of MTN (MADAPI), and its `code` in `providers`.
pub const MTN_CM: &str = "mtn_cm";
/// `Provider.key` of AWS End User Messaging SMS, and its `code` in `providers`.
pub const AWS_SMS: &str = "aws_sms";

/// Orange's API host. The default of the old `ORANGE_CM_BASE_URL`, and what
/// `OrangeCmConfig::production` bakes in (a test holds the two together).
pub const ORANGE_DEFAULT_BASE_URL: &str = "https://api.orange.com";
/// MADAPI's host. The default of the old `MTN_BASE_URL`.
pub const MTN_DEFAULT_BASE_URL: &str = "https://api.mtn.com";

/// Connect and request timeouts every adapter is built with, as the binaries
/// always did.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// The shape of the document: what serde reads. Everything optional, so a
// missing key becomes a `MissingProviderSetting` that names it rather than a
// serde error that does not name the provider.
// ---------------------------------------------------------------------------

/// A number written as a number or as the text a `${VAR}` placeholder
/// resolves to.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Number {
    Int(i64),
    Float(f64),
    Text(String),
}

impl Number {
    fn text(&self) -> String {
        match self {
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Text(value) => value.trim().to_owned(),
        }
    }

    fn as_f64(&self) -> Option<f64> {
        self.text().parse().ok()
    }

    fn as_decimal(&self) -> Option<Decimal> {
        self.text().parse().ok()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHost {
    url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOrangeSettings {
    sender_number: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOAuthCredentials {
    client_id: Option<String>,
    client_secret: Option<Secret>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMtnSettings {
    service_code: Option<String>,
    sender_id: Option<String>,
    tps_ceiling: Option<Number>,
    cost_per_segment_xaf: Option<Number>,
    #[serde(default)]
    supports_alphanumeric_sender: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAwsSettings {
    region: Option<String>,
    configuration_set: Option<String>,
    tps_ceiling: Option<Number>,
    cost_per_segment_xaf: Option<Number>,
    origination_identity: Option<String>,
    message_type: Option<String>,
    #[serde(default)]
    supports_alphanumeric_sender: bool,
    time_to_live_seconds: Option<u64>,
    events_topic_arn: Option<String>,
    fake_sns_cert_base: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAwsCredentials {
    access_key_id: Option<String>,
    secret_access_key: Option<Secret>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawOrangeCm {
    #[serde(default)]
    host: RawHost,
    #[serde(default)]
    settings: RawOrangeSettings,
    #[serde(default)]
    credentials: RawOAuthCredentials,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMtnCm {
    #[serde(default)]
    host: RawHost,
    #[serde(default)]
    settings: RawMtnSettings,
    #[serde(default)]
    credentials: RawOAuthCredentials,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAwsSms {
    #[serde(default)]
    host: RawHost,
    #[serde(default)]
    settings: RawAwsSettings,
    #[serde(default)]
    credentials: RawAwsCredentials,
}

/// One entry of `providers:`, tagged by its `code`.
#[derive(Debug, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub(crate) enum RawProvider {
    OrangeCm(RawOrangeCm),
    MtnCm(RawMtnCm),
    AwsSms(RawAwsSms),
}

// ---------------------------------------------------------------------------
// What the binaries read: validated, with nothing optional that the adapter
// needs.
// ---------------------------------------------------------------------------

/// The configured providers. An absent entry is a provider this deployment does
/// not run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Providers {
    /// `code: orange_cm`.
    pub orange_cm: Option<OrangeCm>,
    /// `code: mtn_cm`.
    pub mtn_cm: Option<MtnCm>,
    /// `code: aws_sms`.
    pub aws_sms: Option<AwsSms>,
}

impl Providers {
    /// Whether the deployment configures any provider at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.orange_cm.is_none() && self.mtn_cm.is_none() && self.aws_sms.is_none()
    }
}

/// Orange Cameroon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrangeCm {
    /// The `OAuth2` `client_credentials` client id.
    pub client_id: String,
    /// The matching secret.
    pub client_secret: Secret,
    /// E.164, without the `tel:` scheme.
    pub sender_number: String,
    /// The API host.
    pub base_url: String,
}

impl OrangeCm {
    /// The adapter's config.
    #[must_use]
    pub fn to_provider_config(&self) -> sms_provider_orange_cm::OrangeCmConfig {
        let mut config = sms_provider_orange_cm::OrangeCmConfig::production(
            self.client_id.clone(),
            self.client_secret.expose().to_owned(),
            self.sender_number.clone(),
        );
        config.base_url.clone_from(&self.base_url);
        config
    }
}

/// MTN Cameroon through MADAPI.
#[derive(Debug, Clone, PartialEq)]
pub struct MtnCm {
    /// The `OAuth2` `client_credentials` client id.
    pub client_id: String,
    /// The matching secret.
    pub client_secret: Secret,
    /// The approved short code every submit carries.
    pub service_code: String,
    /// The approved alphanumeric sender, a fallback when a message has none.
    pub sender_id: Option<String>,
    /// MADAPI's host.
    pub base_url: String,
    /// Messages per second this contract allows.
    pub tps_ceiling: f64,
    /// What one segment costs, in XAF.
    pub cost_per_segment_xaf: Decimal,
    /// Whether an alphanumeric sender is registered and approved.
    pub supports_alphanumeric_sender: bool,
}

impl MtnCm {
    /// The adapter's config.
    #[must_use]
    pub fn to_provider_config(&self) -> sms_provider_mtn::MtnConfig {
        sms_provider_mtn::MtnConfig {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.expose().to_owned(),
            service_code: self.service_code.clone(),
            sender_id: self.sender_id.clone(),
            base_url: self.base_url.clone(),
            tps_ceiling: self.tps_ceiling,
            cost_per_segment_xaf: self.cost_per_segment_xaf,
            supports_alphanumeric_sender: self.supports_alphanumeric_sender,
            connect_timeout: CONNECT_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
        }
    }

    /// The config `mtn-subscribe-dlr` builds: the four values a subscription
    /// call reads, and placeholders for the commercial terms it never does.
    #[must_use]
    pub fn to_subscription_config(&self) -> sms_provider_mtn::MtnConfig {
        sms_provider_mtn::MtnConfig::for_subscription_only(
            self.client_id.clone(),
            self.client_secret.expose().to_owned(),
            self.service_code.clone(),
            self.base_url.clone(),
        )
    }
}

/// AWS End User Messaging SMS: a sending half (`sms-worker`) and a receiving
/// half (`sms-gateway`), each optional, at least one present.
#[derive(Debug, Clone, PartialEq)]
pub struct AwsSms {
    /// What `dispatch` sends with. `None` when the entry only receives.
    pub sender: Option<AwsSender>,
    /// The SNS topic delivery events arrive on. Setting it mounts
    /// `POST /dlr/aws_sms`.
    pub events_topic_arn: Option<String>,
    /// Demo and test builds only: where to fetch SNS signing certificates.
    pub fake_sns_cert_base: Option<String>,
}

/// The six values AWS sending has no safe default for, and the optional rest.
#[derive(Debug, Clone, PartialEq)]
pub struct AwsSender {
    /// The region the account sends from.
    pub region: String,
    /// The IAM access key id.
    pub access_key_id: String,
    /// The IAM secret access key.
    pub secret_access_key: Secret,
    /// The configuration set whose event destination carries receipts.
    pub configuration_set: String,
    /// A fixed origination identity, when there is one.
    pub origination_identity: Option<String>,
    /// Transactional or promotional.
    pub message_type: sms_provider_aws_sms::MessageType,
    /// Message parts per second the account may send.
    pub tps_ceiling: f64,
    /// What one segment costs, in XAF.
    pub cost_per_segment_xaf: Decimal,
    /// Whether the account sends under alphanumeric sender IDs.
    pub supports_alphanumeric_sender: bool,
    /// An endpoint override (a VPC endpoint, a test double).
    pub endpoint_url: Option<String>,
    /// How long AWS keeps trying to deliver one message.
    pub time_to_live: Option<Duration>,
}

impl AwsSender {
    /// The adapter's config.
    #[must_use]
    pub fn to_provider_config(&self) -> sms_provider_aws_sms::AwsSmsConfig {
        sms_provider_aws_sms::AwsSmsConfig {
            region: self.region.clone(),
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.expose().to_owned(),
            configuration_set: self.configuration_set.clone(),
            origination_identity: self.origination_identity.clone(),
            message_type: self.message_type,
            tps_ceiling: self.tps_ceiling,
            cost_per_segment_xaf: self.cost_per_segment_xaf,
            supports_alphanumeric_sender: self.supports_alphanumeric_sender,
            endpoint_url: self.endpoint_url.clone(),
            time_to_live: self.time_to_live,
            connect_timeout: CONNECT_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
        }
    }
}

// ---------------------------------------------------------------------------
// Validation: the raw entries into `Providers`.
// ---------------------------------------------------------------------------

/// The key of a setting for an error message: `providers[code=mtn_cm].settings.x`.
fn key(code: &str, path: &str) -> String {
    format!("providers[code={code}].{path}")
}

fn present(code: &str, path: &str, value: Option<String>) -> Result<String, ConfigError> {
    value.filter(|text| !text.trim().is_empty()).ok_or_else(|| {
        ConfigError::MissingProviderSetting {
            provider: code.to_owned(),
            key: path.to_owned(),
        }
    })
}

fn present_secret(code: &str, path: &str, value: Option<Secret>) -> Result<Secret, ConfigError> {
    value
        .filter(|secret| !secret.is_blank())
        .ok_or_else(|| ConfigError::MissingProviderSetting {
            provider: code.to_owned(),
            key: path.to_owned(),
        })
}

fn blank_is_none(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

fn positive_f64(code: &str, path: &str, value: Option<&Number>) -> Result<f64, ConfigError> {
    let number = value.ok_or_else(|| ConfigError::MissingProviderSetting {
        provider: code.to_owned(),
        key: path.to_owned(),
    })?;
    let parsed = number
        .as_f64()
        .filter(|parsed| parsed.is_finite() && *parsed > 0.0)
        .ok_or_else(|| ConfigError::InvalidValue {
            key: key(code, path),
            reason: format!("{} is not a positive number", number.text()),
        })?;
    Ok(parsed)
}

fn non_negative_decimal(
    code: &str,
    path: &str,
    value: Option<&Number>,
) -> Result<Decimal, ConfigError> {
    let number = value.ok_or_else(|| ConfigError::MissingProviderSetting {
        provider: code.to_owned(),
        key: path.to_owned(),
    })?;
    number
        .as_decimal()
        .filter(|parsed| !parsed.is_sign_negative())
        .ok_or_else(|| ConfigError::InvalidValue {
            key: key(code, path),
            reason: format!("{} is not a non-negative decimal", number.text()),
        })
}

impl RawOrangeCm {
    fn validate(self) -> Result<OrangeCm, ConfigError> {
        Ok(OrangeCm {
            client_id: present(
                ORANGE_CM,
                "credentials.client_id",
                self.credentials.client_id,
            )?,
            client_secret: present_secret(
                ORANGE_CM,
                "credentials.client_secret",
                self.credentials.client_secret,
            )?,
            sender_number: present(
                ORANGE_CM,
                "settings.sender_number",
                self.settings.sender_number,
            )?,
            base_url: blank_is_none(self.host.url)
                .unwrap_or_else(|| ORANGE_DEFAULT_BASE_URL.to_owned()),
        })
    }
}

impl RawMtnCm {
    fn validate(self) -> Result<MtnCm, ConfigError> {
        let settings = self.settings;
        Ok(MtnCm {
            client_id: present(MTN_CM, "credentials.client_id", self.credentials.client_id)?,
            client_secret: present_secret(
                MTN_CM,
                "credentials.client_secret",
                self.credentials.client_secret,
            )?,
            service_code: present(MTN_CM, "settings.service_code", settings.service_code)?,
            sender_id: blank_is_none(settings.sender_id),
            base_url: blank_is_none(self.host.url)
                .unwrap_or_else(|| MTN_DEFAULT_BASE_URL.to_owned()),
            tps_ceiling: positive_f64(
                MTN_CM,
                "settings.tps_ceiling",
                settings.tps_ceiling.as_ref(),
            )?,
            cost_per_segment_xaf: non_negative_decimal(
                MTN_CM,
                "settings.cost_per_segment_xaf",
                settings.cost_per_segment_xaf.as_ref(),
            )?,
            supports_alphanumeric_sender: settings.supports_alphanumeric_sender,
        })
    }
}

impl RawAwsSms {
    /// The sending half is all-or-none over the six values with no safe
    /// default, exactly as `sms-worker`'s flags were. Naming any of them
    /// configures sending, and the first one missing is named.
    fn validate(self) -> Result<AwsSms, ConfigError> {
        let RawAwsSettings {
            region,
            configuration_set,
            tps_ceiling,
            cost_per_segment_xaf,
            origination_identity,
            message_type,
            supports_alphanumeric_sender,
            time_to_live_seconds,
            events_topic_arn,
            fake_sns_cert_base,
        } = self.settings;
        let credentials = self.credentials;
        let sends = region.is_some()
            || configuration_set.is_some()
            || tps_ceiling.is_some()
            || cost_per_segment_xaf.is_some()
            || credentials.access_key_id.is_some()
            || credentials.secret_access_key.is_some();
        let events_topic_arn = blank_is_none(events_topic_arn);
        if !sends && events_topic_arn.is_none() {
            return Err(ConfigError::ProviderConfiguresNothing);
        }
        if !sends {
            // Settings of the sending half with no sending half to apply to are
            // dormant configuration, which a deployment should not carry.
            let orphan = [
                ("host.url", blank_is_none(self.host.url.clone()).is_some()),
                (
                    "settings.origination_identity",
                    blank_is_none(origination_identity.clone()).is_some(),
                ),
                (
                    "settings.message_type",
                    blank_is_none(message_type.clone()).is_some(),
                ),
                (
                    "settings.time_to_live_seconds",
                    time_to_live_seconds.is_some(),
                ),
                (
                    "settings.supports_alphanumeric_sender",
                    supports_alphanumeric_sender,
                ),
            ]
            .into_iter()
            .find_map(|(path, set)| set.then_some(path));
            if let Some(path) = orphan {
                return Err(ConfigError::InvalidValue {
                    key: key(AWS_SMS, path),
                    reason: "it configures sending, and this entry sets none of the sending keys"
                        .to_owned(),
                });
            }
        }
        let sender = if sends {
            let message_type = match blank_is_none(message_type) {
                None => sms_provider_aws_sms::MessageType::Transactional,
                Some(text) => text
                    .parse()
                    .map_err(|reason: String| ConfigError::InvalidValue {
                        key: key(AWS_SMS, "settings.message_type"),
                        reason,
                    })?,
            };
            Some(AwsSender {
                region: present(AWS_SMS, "settings.region", region)?,
                access_key_id: present(
                    AWS_SMS,
                    "credentials.access_key_id",
                    credentials.access_key_id,
                )?,
                secret_access_key: present_secret(
                    AWS_SMS,
                    "credentials.secret_access_key",
                    credentials.secret_access_key,
                )?,
                configuration_set: present(
                    AWS_SMS,
                    "settings.configuration_set",
                    configuration_set,
                )?,
                origination_identity: blank_is_none(origination_identity),
                message_type,
                tps_ceiling: positive_f64(AWS_SMS, "settings.tps_ceiling", tps_ceiling.as_ref())?,
                cost_per_segment_xaf: non_negative_decimal(
                    AWS_SMS,
                    "settings.cost_per_segment_xaf",
                    cost_per_segment_xaf.as_ref(),
                )?,
                supports_alphanumeric_sender,
                endpoint_url: blank_is_none(self.host.url),
                time_to_live: time_to_live_seconds.map(Duration::from_secs),
            })
        } else {
            None
        };
        Ok(AwsSms {
            sender,
            events_topic_arn,
            fake_sns_cert_base: blank_is_none(fake_sns_cert_base),
        })
    }
}

/// Turns the raw list into [`Providers`], refusing a code configured twice and
/// every entry that is incomplete.
pub(crate) fn validate_providers(raw: Vec<RawProvider>) -> Result<Providers, ConfigError> {
    let mut providers = Providers::default();
    for entry in raw {
        match entry {
            RawProvider::OrangeCm(raw) => {
                if providers.orange_cm.replace(raw.validate()?).is_some() {
                    return Err(ConfigError::DuplicateProvider(ORANGE_CM.to_owned()));
                }
            }
            RawProvider::MtnCm(raw) => {
                if providers.mtn_cm.replace(raw.validate()?).is_some() {
                    return Err(ConfigError::DuplicateProvider(MTN_CM.to_owned()));
                }
            }
            RawProvider::AwsSms(raw) => {
                if providers.aws_sms.replace(raw.validate()?).is_some() {
                    return Err(ConfigError::DuplicateProvider(AWS_SMS.to_owned()));
                }
            }
        }
    }
    Ok(providers)
}

impl AwsSms {
    /// Every URL the entry carries, with the key it was written under, for the
    /// livemode host rules.
    pub(crate) fn hosts(&self) -> Vec<(String, &str)> {
        let mut hosts = Vec::new();
        if let Some(sender) = &self.sender
            && let Some(url) = &sender.endpoint_url
        {
            hosts.push((key(AWS_SMS, "host.url"), url.as_str()));
        }
        hosts
    }
}
