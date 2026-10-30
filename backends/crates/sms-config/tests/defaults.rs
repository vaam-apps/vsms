//! The YAML defaults are the defaults the clap flags had.
//!
//! VSC-1 moved the flags into `config/application.yml`; "defaults unchanged" is
//! this file. The left-hand column is the old default, with where it was
//! declared at `a6ac007` (the commit before VSC-1); the right-hand side is read
//! out of the committed base file. Changing either without the other fails here,
//! so a default moves on purpose or not at all.

use std::path::Path;

use sms_config::{Binary, Config, DEFAULT_PROFILE, MTN_DEFAULT_BASE_URL, ORANGE_DEFAULT_BASE_URL};

mod common;

fn base_config() -> Config {
    Config::load_with_env(
        Some(&common::committed_base_alone()),
        DEFAULT_PROFILE,
        Binary::Gateway,
        &|name| (name == "SMS_HASH_PEPPER").then(|| "p".repeat(48)),
    )
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
    let config =
        Config::load_with_env(Some(&dir), "valid-defaults", Binary::Gateway, &env).unwrap();
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
    let aws = Config::load_with_env(Some(&dir), "valid-livemode", Binary::Gateway, &env)
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

#[test]
fn the_committed_base_leaves_auth_methods_absent_so_every_method_is_permitted() {
    // VSA-1: a deployment that never wrote the key gets what it always got. The
    // base file carries the key as a comment only; writing it there would turn
    // the shipped behaviour off for every profile.
    let config = base_config();
    assert!(!config.auth_methods.is_explicit());
    assert_eq!(
        config.auth_methods.permitted().count(),
        sms_config::AuthMethod::ALL.len()
    );
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/application.yml"),
    )
    .unwrap();
    assert!(
        !text.lines().any(|line| line.starts_with("auth:")),
        "config/application.yml must carry `auth:` as a comment, not as a key"
    );
}

/// The commented examples in `config/application.yml` are claims about what a
/// deployment may write, and a comment is the one place nothing would notice them
/// drifting. Uncommented, the `op:` and `human_login:` examples (the second with
/// `sso:` nested in it) load, and serve both methods.
#[test]
fn the_commented_op_and_human_login_examples_load_once_uncommented_and_serve_both() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/application.yml"),
    )
    .unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut overlay = String::new();
    for block in ["# op:", "# human_login:"] {
        let start = lines
            .iter()
            .position(|line| *line == block)
            .unwrap_or_else(|| panic!("config/application.yml has no commented {block} example"));
        overlay.push_str(&lines[start][2..]);
        overlay.push('\n');
        for line in &lines[start + 1..] {
            // The example is the run of `#   ` lines (an indented continuation).
            let Some(rest) = line.strip_prefix("# ") else {
                break;
            };
            if !rest.starts_with("  ") {
                break;
            }
            overlay.push_str(rest);
            overlay.push('\n');
        }
    }
    let dir = std::env::temp_dir().join(format!("sms-config-example-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(common::committed_base_alone(), dir.join("application.yml")).unwrap();
    std::fs::write(dir.join("application-example.yml"), &overlay).unwrap();

    let config = Config::load_with_env(
        Some(&dir.join("application.yml")),
        "example",
        Binary::Gateway,
        &|name| matches!(name, "SMS_HASH_PEPPER" | "SMS_SSO_CLIENT_SECRET").then(|| "p".repeat(48)),
    )
    .unwrap_or_else(|error| panic!("the commented examples do not load: {error}\n{overlay}"));
    assert!(config.auth_methods.machine_tokens_served(), "{overlay}");
    assert!(config.auth_methods.human_login_served(), "{overlay}");
    let human = config.human_login.clone().expect("the block is read");
    assert_eq!(human.console_client.client_id, "sms-console");
    assert_eq!(human.login_rate_limit.per_email.burst, 5);
    assert_eq!(human.login_rate_limit.aggregate.burst, 10);
    // VSA-4c: the `sso:` example nested in it loads too, with its secret from the
    // placeholder the comment writes.
    let sso = config
        .sso()
        .unwrap_or_else(|| panic!("no sso example read\n{overlay}"));
    assert_eq!(sso.issuer, "https://auth.example.org/realms/staff");
    assert_eq!(sso.jit_role, "support");
    assert_eq!(sso.accepted_amr, ["otp"]);
    assert_eq!(sso.rate_limit.burst, 30);
    assert_eq!(sso.client_secret.expose(), "p".repeat(48));
    std::fs::remove_dir_all(&dir).ok();
}
