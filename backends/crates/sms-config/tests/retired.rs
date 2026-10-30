//! `RETIRED` is complete, and every entry points at a real YAML key.
//!
//! The first test holds the list to what the binaries declared before VSC-1,
//! written out here a second time on purpose: `RETIRED` is read by the loader,
//! by both binaries' tests and by the cutover tests, so a name dropped from it
//! would silently stop being refused everywhere at once, and none of those
//! readers could notice. This copy is the independent witness.
//!
//! The second makes the mapping real: for each entry it writes the YAML key into
//! an overlay and loads it. An unknown key is a `Shape` error, so a `key` that
//! names nothing the model reads (a typo in the table, or a field renamed
//! without it) fails here instead of misdirecting an operator at boot.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sms_config::{Binary, Config, ConfigError, RETIRED};

/// Every `#[arg(long, env = ...)]` that `sms-gateway serve`,
/// `sms-gateway mtn-subscribe-dlr` and `sms-worker` declared at `a6ac007`, minus
/// the process plumbing that stays (`DATABASE_URL`, the listen and metrics
/// addresses, the envelope key file paths, `SMS_WORKER_ROLES`, `SMS_WORKER_ID`).
/// `(variable, flag)`.
const OLD_CLI: [(&str, &str); 44] = [
    // serve: backends/apps/sms-gateway/src/commands/serve.rs
    ("SMS_DB_MAX_CONNECTIONS", "--max-connections"),
    ("SMS_HASH_PEPPER", "--hash-pepper"),
    ("SMS_IDEMPOTENCY_TTL_SECS", "--idempotency-ttl-secs"),
    ("SMS_RATE_LIMIT_BURST", "--rate-limit-burst"),
    (
        "SMS_RATE_LIMIT_REFILL_PER_SECOND",
        "--rate-limit-refill-per-second",
    ),
    ("SMS_SOURCE_RATE_LIMIT_BURST", "--source-rate-limit-burst"),
    (
        "SMS_SOURCE_RATE_LIMIT_REFILL_PER_SECOND",
        "--source-rate-limit-refill-per-second",
    ),
    ("ORANGE_CM_CLIENT_ID", "--orange-client-id"),
    ("ORANGE_CM_CLIENT_SECRET", "--orange-client-secret"),
    ("ORANGE_CM_SENDER_NUMBER", "--orange-sender-number"),
    ("ORANGE_CM_BASE_URL", "--orange-base-url"),
    ("MTN_CLIENT_ID", "--mtn-client-id"),
    ("MTN_CLIENT_SECRET", "--mtn-client-secret"),
    ("MTN_SERVICE_CODE", "--mtn-service-code"),
    ("MTN_SENDER_ID", "--mtn-sender-id"),
    ("MTN_BASE_URL", "--mtn-base-url"),
    ("MTN_TPS_CEILING", "--mtn-tps-ceiling"),
    ("MTN_COST_PER_SEGMENT_XAF", "--mtn-cost-per-segment-xaf"),
    (
        "MTN_SUPPORTS_ALPHANUMERIC_SENDER",
        "--mtn-supports-alphanumeric-sender",
    ),
    ("AWS_SMS_EVENTS_TOPIC_ARN", "--aws-sms-events-topic-arn"),
    ("AWS_SMS_FAKE_SNS_CERT_BASE", "--aws-sms-fake-sns-cert-base"),
    // sms-worker: backends/apps/sms-worker/src/main.rs (the rest are shared)
    ("SMS_WORKER_DB_MAX_CONNECTIONS", "--db-max-connections"),
    ("AWS_SMS_REGION", "--aws-sms-region"),
    ("AWS_SMS_ACCESS_KEY_ID", "--aws-sms-access-key-id"),
    ("AWS_SMS_SECRET_ACCESS_KEY", "--aws-sms-secret-access-key"),
    ("AWS_SMS_CONFIGURATION_SET", "--aws-sms-configuration-set"),
    ("AWS_SMS_TPS_CEILING", "--aws-sms-tps-ceiling"),
    (
        "AWS_SMS_COST_PER_SEGMENT_XAF",
        "--aws-sms-cost-per-segment-xaf",
    ),
    (
        "AWS_SMS_ORIGINATION_IDENTITY",
        "--aws-sms-origination-identity",
    ),
    ("AWS_SMS_MESSAGE_TYPE", "--aws-sms-message-type"),
    (
        "AWS_SMS_SUPPORTS_ALPHANUMERIC_SENDER",
        "--aws-sms-supports-alphanumeric-sender",
    ),
    ("AWS_SMS_ENDPOINT_URL", "--aws-sms-endpoint-url"),
    (
        "AWS_SMS_TIME_TO_LIVE_SECONDS",
        "--aws-sms-time-to-live-seconds",
    ),
    // serve, 29f93c4^: the single-sign-on flags VSA-4c retires as `human_login.sso`
    ("SMS_SSO_ISSUER", "--sso-issuer"),
    ("SMS_SSO_CLIENT_ID", "--sso-client-id"),
    ("SMS_SSO_CLIENT_SECRET", "--sso-client-secret"),
    ("SMS_SSO_REDIRECT_URI", "--sso-redirect-uri"),
    ("SMS_SSO_SCOPES", "--sso-scopes"),
    ("SMS_SSO_REQUIRED_GROUP", "--sso-required-group"),
    ("SMS_SSO_GROUPS_CLAIM", "--sso-groups-claim"),
    ("SMS_SSO_ACCEPTED_ACR", "--sso-accepted-acr"),
    ("SMS_SSO_ACCEPTED_AMR", "--sso-accepted-amr"),
    ("SMS_SSO_JIT_ROLE", "--sso-jit-role"),
    (
        "SMS_SSO_JWKS_MIN_REFRESH_SECS",
        "--sso-jwks-min-refresh-secs",
    ),
];

#[test]
fn retired_is_exactly_what_the_binaries_declared() {
    let expected: BTreeSet<(&str, &str)> = OLD_CLI.into_iter().collect();
    let actual: BTreeSet<(&str, &str)> = RETIRED
        .iter()
        .map(|retired| (retired.env, retired.flag))
        .collect();
    assert_eq!(
        expected.difference(&actual).collect::<Vec<_>>(),
        Vec::<&(&str, &str)>::new(),
        "declared before VSC-1 and no longer refused"
    );
    assert_eq!(
        actual.difference(&expected).collect::<Vec<_>>(),
        Vec::<&(&str, &str)>::new(),
        "refused, but never declared"
    );
}

fn base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/application.yml")
}

/// The overlay that sets `key` alone, with a value of the type the model reads.
fn overlay_setting(key: &str) -> String {
    let leaf = key.rsplit('.').next().unwrap();
    let value = match leaf {
        "max_connections"
        | "gateway_max_connections"
        | "worker_max_connections"
        | "burst"
        | "idempotency_ttl_secs"
        | "time_to_live_seconds"
        | "tps_ceiling" => "5",
        "refill_per_second" | "cost_per_segment_xaf" => "5.5",
        "supports_alphanumeric_sender" => "true",
        _ => "\"x\"",
    };
    if let Some(rest) = key.strip_prefix("providers[code=") {
        let (code, path) = rest.split_once("].").unwrap();
        let mut lines = vec![format!("providers:\n  - code: {code}")];
        let parts: Vec<&str> = path.split('.').collect();
        for (depth, part) in parts.iter().enumerate() {
            let indent = "    ".to_owned() + &"  ".repeat(depth);
            if depth + 1 == parts.len() {
                lines.push(format!("{indent}{part}: {value}"));
            } else {
                lines.push(format!("{indent}{part}:"));
            }
        }
        return lines.join("\n") + "\n";
    }
    let parts: Vec<&str> = key.split('.').collect();
    let mut lines = Vec::new();
    for (depth, part) in parts.iter().enumerate() {
        let indent = "  ".repeat(depth);
        if depth + 1 == parts.len() {
            lines.push(format!("{indent}{part}: {value}"));
        } else {
            lines.push(format!("{indent}{part}:"));
        }
    }
    lines.join("\n") + "\n"
}

#[test]
fn every_retired_key_is_a_key_the_model_reads() {
    let dir = std::env::temp_dir().join(format!("sms-config-retired-keys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(base(), dir.join("application.yml")).unwrap();
    for retired in RETIRED {
        std::fs::write(
            dir.join("application-probe.yml"),
            overlay_setting(retired.key),
        )
        .unwrap();
        let result = Config::load_with_env(
            Some(&dir.join("application.yml")),
            "probe",
            Binary::Gateway,
            &|name| (name == "SMS_HASH_PEPPER").then(|| "p".repeat(48)),
        );
        // Anything but an `unknown field` means the key was understood: it may
        // still be an incomplete provider or a block missing its siblings (a
        // `human_login.sso.*` key alone is), which are different rules.
        if let Err(ConfigError::Shape(message)) = &result
            && message.contains("unknown field")
        {
            panic!(
                "{} maps to `{}`, which the model does not read: {message}\n{}",
                retired.env,
                retired.key,
                overlay_setting(retired.key)
            );
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}
