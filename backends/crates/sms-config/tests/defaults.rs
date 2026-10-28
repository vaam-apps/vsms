//! The YAML defaults are the defaults the clap flags had.
//!
//! VSC-1 moved the flags into `config/application.yml`; "defaults unchanged" is
//! this file. The left-hand column is the old default, with where it was
//! declared at `a6ac007` (the commit before VSC-1); the right-hand side is read
//! out of the committed base file. Changing either without the other fails here,
//! so a default moves on purpose or not at all.

use std::path::{Path, PathBuf};

use sms_config::{Config, MTN_DEFAULT_BASE_URL, ORANGE_DEFAULT_BASE_URL};

fn base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/application.yml")
}

fn base_config() -> Config {
    Config::load_with_env(Some(&base()), "no-such-profile", &|name| {
        (name == "SMS_HASH_PEPPER").then(|| "p".repeat(48))
    })
    .expect("the committed base file loads on its own")
}

#[test]
fn the_pool_sizes_are_the_old_flag_defaults() {
    let config = base_config();
    // sms-gateway serve: `SMS_DB_MAX_CONNECTIONS`, `default_value_t = 10`.
    assert_eq!(config.database.gateway_max_connections, 10);
    // sms-worker: `SMS_WORKER_DB_MAX_CONNECTIONS`, `default_value_t = 10`.
    assert_eq!(config.database.worker_max_connections, 10);
}

#[test]
fn the_idempotency_ttl_is_the_old_flag_default() {
    // `SMS_IDEMPOTENCY_TTL_SECS`, `default_value_t = 24 * 60 * 60`.
    assert_eq!(base_config().api.idempotency_ttl_secs, 24 * 60 * 60);
}

#[test]
fn the_rate_limits_are_the_old_flag_defaults() {
    let api = base_config().api;
    // `SMS_RATE_LIMIT_BURST` 120, `SMS_RATE_LIMIT_REFILL_PER_SECOND` 2.0.
    assert_eq!(api.rate_limit.burst, 120);
    assert!((api.rate_limit.refill_per_second - 2.0).abs() < f64::EPSILON);
    // `SMS_SOURCE_RATE_LIMIT_BURST` 1200, `..._REFILL_PER_SECOND` 10.0.
    assert_eq!(api.source_rate_limit.burst, 1200);
    assert!((api.source_rate_limit.refill_per_second - 10.0).abs() < f64::EPSILON);
}

#[test]
fn the_provider_hosts_are_the_old_flag_defaults() {
    // `ORANGE_CM_BASE_URL`, `default_value = "https://api.orange.com"`.
    assert_eq!(ORANGE_DEFAULT_BASE_URL, "https://api.orange.com");
    // `MTN_BASE_URL`, `default_value = "https://api.mtn.com"`.
    assert_eq!(MTN_DEFAULT_BASE_URL, "https://api.mtn.com");
}

#[test]
fn the_orange_default_is_what_the_adapter_itself_defaults_to() {
    let adapter = sms_provider_orange_cm::OrangeCmConfig::production(
        String::new(),
        String::new(),
        String::new(),
    );
    assert_eq!(adapter.base_url, ORANGE_DEFAULT_BASE_URL);
}

#[test]
fn the_optional_provider_flags_default_as_they_did() {
    use sms_provider_aws_sms::MessageType;
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/base.yml");
    // Exactly the variables each fixture reads: a retired variable set and not
    // read is refused.
    let env_of = |names: &'static [&'static str]| {
        move |name: &str| names.contains(&name).then(|| "x".repeat(48))
    };
    let env = env_of(&[
        "SMS_HASH_PEPPER",
        "ORANGE_CM_CLIENT_SECRET",
        "MTN_CLIENT_SECRET",
    ]);
    let config = Config::load_with_env(Some(&dir), "valid-defaults", &env).unwrap();
    // `MTN_SUPPORTS_ALPHANUMERIC_SENDER`, `default_value_t = false`.
    assert!(
        !config
            .providers
            .mtn_cm
            .unwrap()
            .supports_alphanumeric_sender
    );
    // `AWS_SMS_MESSAGE_TYPE`, `default_value = "transactional"`, and
    // `AWS_SMS_SUPPORTS_ALPHANUMERIC_SENDER`, `default_value_t = false`.
    let env = env_of(&[
        "SMS_HASH_PEPPER",
        "ORANGE_CM_CLIENT_SECRET",
        "MTN_CLIENT_SECRET",
        "AWS_SMS_SECRET_ACCESS_KEY",
    ]);
    let aws = Config::load_with_env(Some(&dir), "valid-livemode", &env)
        .unwrap()
        .providers
        .aws_sms
        .unwrap()
        .sender
        .unwrap();
    assert_eq!(aws.message_type, MessageType::Transactional);
    assert!(!aws.supports_alphanumeric_sender);
    assert_eq!(aws.time_to_live, None);
    assert_eq!(aws.endpoint_url, None);
}
