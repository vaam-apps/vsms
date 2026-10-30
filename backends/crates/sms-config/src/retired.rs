#![doc = include_str!("retired.md")]

use std::collections::BTreeSet;
use std::ffi::OsStr;

use crate::error::ConfigError;

/// One flag-and-variable pair that configured a binary before VSC-1, and the
/// YAML key that does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retired {
    /// The environment variable clap bound to the flag.
    pub env: &'static str,
    /// The flag, with its leading dashes.
    pub flag: &'static str,
    /// The YAML key, relative to the document root.
    pub key: &'static str,
}

const fn retired(env: &'static str, flag: &'static str, key: &'static str) -> Retired {
    Retired { env, flag, key }
}

/// Every flag and variable VSC-1 (and VSA-4c, for the single-sign-on ones) moved into the YAML, in the order the old
/// `--help` listed them. `sms-gateway serve`, `sms-gateway mtn-subscribe-dlr`
/// and `sms-worker` each declared a subset; the union is refused everywhere,
/// because a name that meant "the Orange client id" must not mean anything else
/// in one binary.
pub const RETIRED: &[Retired] = &[
    retired(
        "SMS_DB_MAX_CONNECTIONS",
        "--max-connections",
        "database.gateway_max_connections",
    ),
    retired(
        "SMS_WORKER_DB_MAX_CONNECTIONS",
        "--db-max-connections",
        "database.worker_max_connections",
    ),
    retired("SMS_HASH_PEPPER", "--hash-pepper", "security.hash_pepper"),
    retired(
        "SMS_IDEMPOTENCY_TTL_SECS",
        "--idempotency-ttl-secs",
        "api.idempotency_ttl_secs",
    ),
    retired(
        "SMS_RATE_LIMIT_BURST",
        "--rate-limit-burst",
        "api.rate_limit.burst",
    ),
    retired(
        "SMS_RATE_LIMIT_REFILL_PER_SECOND",
        "--rate-limit-refill-per-second",
        "api.rate_limit.refill_per_second",
    ),
    retired(
        "SMS_SOURCE_RATE_LIMIT_BURST",
        "--source-rate-limit-burst",
        "api.source_rate_limit.burst",
    ),
    retired(
        "SMS_SOURCE_RATE_LIMIT_REFILL_PER_SECOND",
        "--source-rate-limit-refill-per-second",
        "api.source_rate_limit.refill_per_second",
    ),
    retired(
        "ORANGE_CM_CLIENT_ID",
        "--orange-client-id",
        "providers[code=orange_cm].credentials.client_id",
    ),
    retired(
        "ORANGE_CM_CLIENT_SECRET",
        "--orange-client-secret",
        "providers[code=orange_cm].credentials.client_secret",
    ),
    retired(
        "ORANGE_CM_SENDER_NUMBER",
        "--orange-sender-number",
        "providers[code=orange_cm].settings.sender_number",
    ),
    retired(
        "ORANGE_CM_BASE_URL",
        "--orange-base-url",
        "providers[code=orange_cm].host.url",
    ),
    retired(
        "MTN_CLIENT_ID",
        "--mtn-client-id",
        "providers[code=mtn_cm].credentials.client_id",
    ),
    retired(
        "MTN_CLIENT_SECRET",
        "--mtn-client-secret",
        "providers[code=mtn_cm].credentials.client_secret",
    ),
    retired(
        "MTN_SERVICE_CODE",
        "--mtn-service-code",
        "providers[code=mtn_cm].settings.service_code",
    ),
    retired(
        "MTN_SENDER_ID",
        "--mtn-sender-id",
        "providers[code=mtn_cm].settings.sender_id",
    ),
    retired(
        "MTN_BASE_URL",
        "--mtn-base-url",
        "providers[code=mtn_cm].host.url",
    ),
    retired(
        "MTN_TPS_CEILING",
        "--mtn-tps-ceiling",
        "providers[code=mtn_cm].settings.tps_ceiling",
    ),
    retired(
        "MTN_COST_PER_SEGMENT_XAF",
        "--mtn-cost-per-segment-xaf",
        "providers[code=mtn_cm].settings.cost_per_segment_xaf",
    ),
    retired(
        "MTN_SUPPORTS_ALPHANUMERIC_SENDER",
        "--mtn-supports-alphanumeric-sender",
        "providers[code=mtn_cm].settings.supports_alphanumeric_sender",
    ),
    retired(
        "AWS_SMS_REGION",
        "--aws-sms-region",
        "providers[code=aws_sms].settings.region",
    ),
    retired(
        "AWS_SMS_ACCESS_KEY_ID",
        "--aws-sms-access-key-id",
        "providers[code=aws_sms].credentials.access_key_id",
    ),
    retired(
        "AWS_SMS_SECRET_ACCESS_KEY",
        "--aws-sms-secret-access-key",
        "providers[code=aws_sms].credentials.secret_access_key",
    ),
    retired(
        "AWS_SMS_CONFIGURATION_SET",
        "--aws-sms-configuration-set",
        "providers[code=aws_sms].settings.configuration_set",
    ),
    retired(
        "AWS_SMS_TPS_CEILING",
        "--aws-sms-tps-ceiling",
        "providers[code=aws_sms].settings.tps_ceiling",
    ),
    retired(
        "AWS_SMS_COST_PER_SEGMENT_XAF",
        "--aws-sms-cost-per-segment-xaf",
        "providers[code=aws_sms].settings.cost_per_segment_xaf",
    ),
    retired(
        "AWS_SMS_ORIGINATION_IDENTITY",
        "--aws-sms-origination-identity",
        "providers[code=aws_sms].settings.origination_identity",
    ),
    retired(
        "AWS_SMS_MESSAGE_TYPE",
        "--aws-sms-message-type",
        "providers[code=aws_sms].settings.message_type",
    ),
    retired(
        "AWS_SMS_SUPPORTS_ALPHANUMERIC_SENDER",
        "--aws-sms-supports-alphanumeric-sender",
        "providers[code=aws_sms].settings.supports_alphanumeric_sender",
    ),
    retired(
        "AWS_SMS_ENDPOINT_URL",
        "--aws-sms-endpoint-url",
        "providers[code=aws_sms].host.url",
    ),
    retired(
        "AWS_SMS_TIME_TO_LIVE_SECONDS",
        "--aws-sms-time-to-live-seconds",
        "providers[code=aws_sms].settings.time_to_live_seconds",
    ),
    retired(
        "AWS_SMS_EVENTS_TOPIC_ARN",
        "--aws-sms-events-topic-arn",
        "providers[code=aws_sms].settings.events_topic_arn",
    ),
    retired(
        "AWS_SMS_FAKE_SNS_CERT_BASE",
        "--aws-sms-fake-sns-cert-base",
        "providers[code=aws_sms].settings.fake_sns_cert_base",
    ),
    // VSA-4c: the single-sign-on flags of the deleted `serve`, back as
    // `human_login.sso`. The redirect URI is not a key any more: it is derived from
    // `op.issuer`. `SMS_SSO_CLIENT_SECRET` is still the secret's source.
    retired("SMS_SSO_ISSUER", "--sso-issuer", "human_login.sso.issuer"),
    retired(
        "SMS_SSO_CLIENT_ID",
        "--sso-client-id",
        "human_login.sso.client_id",
    ),
    retired(
        "SMS_SSO_CLIENT_SECRET",
        "--sso-client-secret",
        "human_login.sso.client_secret",
    ),
    retired("SMS_SSO_REDIRECT_URI", "--sso-redirect-uri", "op.issuer"),
    retired("SMS_SSO_SCOPES", "--sso-scopes", "human_login.sso.scopes"),
    retired(
        "SMS_SSO_REQUIRED_GROUP",
        "--sso-required-group",
        "human_login.sso.required_group",
    ),
    retired(
        "SMS_SSO_GROUPS_CLAIM",
        "--sso-groups-claim",
        "human_login.sso.groups_claim",
    ),
    retired(
        "SMS_SSO_ACCEPTED_ACR",
        "--sso-accepted-acr",
        "human_login.sso.accepted_acr",
    ),
    retired(
        "SMS_SSO_ACCEPTED_AMR",
        "--sso-accepted-amr",
        "human_login.sso.accepted_amr",
    ),
    retired(
        "SMS_SSO_JIT_ROLE",
        "--sso-jit-role",
        "human_login.sso.jit_role",
    ),
    retired(
        "SMS_SSO_JWKS_MIN_REFRESH_SECS",
        "--sso-jwks-min-refresh-secs",
        "human_login.sso.jwks_min_refresh_secs",
    ),
];

/// Refuses a command line that still carries a retired flag.
///
/// Call it with `std::env::args_os().skip(1)` **before** clap parses: once the
/// flag is gone from the parser, clap's own answer is "unexpected argument",
/// which names no YAML key. `--flag=value` and `--flag value` are both caught.
///
/// # Errors
///
/// [`ConfigError::RetiredFlag`] for the first retired flag found.
pub fn check_args<I, A>(args: I) -> Result<(), ConfigError>
where
    I: IntoIterator<Item = A>,
    A: AsRef<OsStr>,
{
    for arg in args {
        let Some(arg) = arg.as_ref().to_str() else {
            continue;
        };
        let name = arg.split_once('=').map_or(arg, |(name, _)| name);
        if let Some(found) = RETIRED.iter().find(|retired| retired.flag == name) {
            return Err(ConfigError::RetiredFlag {
                flag: found.flag.to_owned(),
                key: found.key.to_owned(),
            });
        }
    }
    Ok(())
}

/// Refuses an environment that still sets a retired variable the YAML does not
/// read. `referenced` is every `${VAR}` name the document writes: a retired
/// variable that appears there is a secret's source, which is how the pepper and
/// the client secrets are still delivered, and is fine.
pub(crate) fn check_env(
    referenced: &BTreeSet<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    for retired in RETIRED {
        if env(retired.env).is_some() && !referenced.contains(retired.env) {
            return Err(ConfigError::RetiredEnv {
                var: retired.env.to_owned(),
                key: retired.key.to_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_retired_name_is_listed_twice() {
        let mut envs = BTreeSet::new();
        let mut flags = BTreeSet::new();
        for retired in RETIRED {
            assert!(envs.insert(retired.env), "{} listed twice", retired.env);
            assert!(flags.insert(retired.flag), "{} listed twice", retired.flag);
            assert!(retired.flag.starts_with("--"), "{}", retired.flag);
        }
    }

    #[test]
    fn both_spellings_of_a_flag_are_caught() {
        for args in [
            vec!["serve", "--orange-client-id", "x"],
            vec!["serve", "--orange-client-id=x"],
        ] {
            assert_eq!(
                check_args(args).unwrap_err(),
                ConfigError::RetiredFlag {
                    flag: "--orange-client-id".to_owned(),
                    key: "providers[code=orange_cm].credentials.client_id".to_owned(),
                }
            );
        }
    }

    #[test]
    fn a_current_flag_with_a_retired_prefix_is_not_a_retired_flag() {
        check_args(["serve", "--listen", "0.0.0.0:8080", "--orange-client"]).unwrap();
    }
}
