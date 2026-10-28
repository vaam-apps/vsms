//! One test per rule, each against its own fixture.
//!
//! A fixture is `tests/fixtures/base-<name>.yml`, loaded over
//! `tests/fixtures/base.yml` with profile `<name>`, so every case also
//! exercises the overlay merge the real profiles rely on. The environment is
//! injected: `std::env::set_var` is `unsafe` and this workspace forbids it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sms_config::{Binary, Config, ConfigError, DEFAULT_PROFILE, RETIRED};

fn base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/base.yml")
}

/// What the placeholders a fixture reads resolve to. Only the variables a
/// document actually reads are set: a retired variable that is set and not read
/// is refused, which is the rule, so an environment padded with names the
/// document never mentions would be refused too.
fn value_of(name: &str) -> String {
    match name {
        "SMS_HASH_PEPPER" => "fixture-pepper-fixture-pepper-fixture-pepper",
        "ORANGE_CM_CLIENT_SECRET" => "orange-secret",
        "MTN_CLIENT_SECRET" => "mtn-secret",
        "AWS_SMS_SECRET_ACCESS_KEY" => "aws-secret",
        "MTN_CLIENT_ID" => "mtn-id-from-env",
        "MTN_SERVICE_CODE" => "131",
        "MTN_TPS" => "7.5",
        "MTN_COST" => "12.50",
        other => return format!("{other}-value"),
    }
    .to_owned()
}

/// The environment for `profile`: every `${NAME}` its documents write, set.
fn env_for(profile: &str) -> BTreeMap<String, String> {
    let dir = base().parent().unwrap().to_path_buf();
    let mut text = std::fs::read_to_string(base()).unwrap();
    if let Ok(overlay) = std::fs::read_to_string(dir.join(format!("base-{profile}.yml"))) {
        // An overlay that writes the pepper itself replaces the base's
        // placeholder, so the document no longer reads SMS_HASH_PEPPER.
        if overlay.contains("hash_pepper:") {
            text = text.replace("${SMS_HASH_PEPPER}", "");
        }
        text.push_str(&overlay);
    }
    let mut env = BTreeMap::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        if !name.is_empty() && name != "A_VARIABLE_NOBODY_SETS" {
            env.insert(name.to_owned(), value_of(name));
        }
        rest = &after[end + 1..];
    }
    env
}

fn env_with(profile: &str, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut env = env_for(profile);
    env.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
    );
    env
}

fn load_in(profile: &str, env: &BTreeMap<String, String>) -> Result<Config, ConfigError> {
    Config::load_with_env(Some(&base()), profile, Binary::Gateway, &|name| {
        env.get(name).cloned()
    })
}

fn load(profile: &str) -> Result<Config, ConfigError> {
    load_in(profile, &env_for(profile))
}

fn refused(profile: &str) -> ConfigError {
    load(profile).expect_err("this fixture breaks a rule and must be refused")
}

// --- the documents that load ----------------------------------------------

#[test]
fn the_base_document_alone_loads_with_no_providers() {
    let config = load(DEFAULT_PROFILE).expect("the base loads when its overlay does not exist");
    assert!(config.providers.is_empty());
    assert_eq!(config.deployment.name, "fixture");
}

#[test]
fn every_provider_loads_with_every_key_set() {
    let config = load("valid-all-providers").unwrap();
    let orange = config.providers.orange_cm.expect("orange");
    assert!(
        orange.client_secret.expose() == "orange-secret",
        "the Orange client secret must reach the adapter as written"
    );
    assert_eq!(orange.base_url, "https://api.orange.example");

    let mtn = config.providers.mtn_cm.expect("mtn");
    assert_eq!(mtn.sender_id.as_deref(), Some("VSMS"));
    assert!((mtn.tps_ceiling - 20.5).abs() < f64::EPSILON);
    assert_eq!(mtn.cost_per_segment_xaf.to_string(), "15.5");
    assert!(mtn.supports_alphanumeric_sender);

    let aws = config.providers.aws_sms.expect("aws");
    let sender = aws.sender.expect("sending half");
    assert_eq!(sender.endpoint_url.as_deref(), Some("https://vpce.example"));
    assert_eq!(sender.time_to_live.map(|ttl| ttl.as_secs()), Some(900));
    assert_eq!(
        format!("{:?}", sender.message_type),
        "Promotional",
        "message_type: promotional"
    );
    assert_eq!(
        aws.events_topic_arn.as_deref(),
        Some("arn:aws:sns:eu-west-1:123456789012:vsms-events")
    );
}

#[test]
fn what_a_provider_leaves_out_takes_the_old_flag_default() {
    let config = load("valid-defaults").unwrap();
    assert_eq!(
        config.providers.orange_cm.unwrap().base_url,
        "https://api.orange.com"
    );
    let mtn = config.providers.mtn_cm.unwrap();
    assert_eq!(mtn.base_url, "https://api.mtn.com");
    assert!(!mtn.supports_alphanumeric_sender);
    assert_eq!(mtn.sender_id, None);
}

#[test]
fn an_aws_entry_may_only_receive() {
    let aws = load("valid-aws-receive-only")
        .unwrap()
        .providers
        .aws_sms
        .unwrap();
    assert!(aws.sender.is_none(), "no sending keys, so no sender");
    assert!(aws.events_topic_arn.is_some());
}

#[test]
fn a_livemode_document_with_https_and_placeholders_loads() {
    let config = load("valid-livemode").unwrap();
    assert!(config.deployment.livemode);
}

#[test]
fn a_number_can_arrive_through_a_placeholder() {
    let mtn = load("valid-placeholder-numbers")
        .unwrap()
        .providers
        .mtn_cm
        .unwrap();
    assert_eq!(mtn.client_id, "mtn-id-from-env");
    assert!((mtn.tps_ceiling - 7.5).abs() < f64::EPSILON);
    assert_eq!(mtn.cost_per_segment_xaf.to_string(), "12.50");
}

#[test]
fn a_sandbox_may_carry_literal_secrets() {
    load("sandbox-literal-secrets-are-fine").unwrap();
}

#[test]
fn a_secret_never_reaches_a_debug_print() {
    let config = load("valid-all-providers").unwrap();
    let printed = format!("{config:?}");
    for (which, secret) in [
        ("the Orange client secret", "orange-secret"),
        ("the MTN client secret", "mtn-secret"),
        ("the AWS secret access key", "aws-secret"),
        ("the hash pepper", "fixture-pepper"),
    ] {
        // The message names the key, never the value or the printout: a
        // failure here is exactly the case where the printout holds a secret.
        assert!(
            !printed.contains(secret),
            "{which} reached the Debug output"
        );
    }
}

// --- file and parse rules --------------------------------------------------

#[test]
fn no_path_is_refused() {
    let env = env_with("dev", &[]);
    let error = Config::load_with_env(None, "dev", Binary::Gateway, &|name| env.get(name).cloned())
        .unwrap_err();
    assert_eq!(error, ConfigError::MissingPath);
}

#[test]
fn a_missing_base_file_is_named() {
    let env = env_with("dev", &[]);
    let typo = Path::new("config/aplication.yml");
    let error = Config::load_with_env(Some(typo), "dev", Binary::Gateway, &|name| {
        env.get(name).cloned()
    })
    .unwrap_err();
    assert_eq!(
        error,
        ConfigError::FileNotFound("config/aplication.yml".to_owned())
    );
}

#[test]
fn an_unparseable_overlay_is_a_load_error() {
    assert!(matches!(
        refused("malformed-yaml"),
        ConfigError::Load { ref path, .. } if path.ends_with("base.yml")
    ));
}

#[test]
fn an_unset_placeholder_is_fatal_and_named() {
    assert_eq!(
        refused("unresolved-placeholder"),
        ConfigError::UnresolvedPlaceholder("A_VARIABLE_NOBODY_SETS".to_owned())
    );
}

#[test]
fn an_unclosed_placeholder_is_malformed() {
    assert!(matches!(
        refused("malformed-placeholder"),
        ConfigError::MalformedPlaceholder { .. }
    ));
}

#[test]
fn a_malformed_placeholder_names_its_key_and_never_the_value() {
    // A sandbox may carry a literal secret, and a literal secret may contain
    // `${`. The error is a log line: it gets the key and the fragment.
    let error = refused("malformed-secret-unclosed");
    let ConfigError::MalformedPlaceholder { key, fragment } = &error else {
        panic!("expected MalformedPlaceholder, got another error");
    };
    assert_eq!(key, "providers[0].credentials.client_secret");
    assert_eq!(fragment, "${ with no closing }");
    for printed in [error.to_string(), format!("{error:?}")] {
        assert!(
            !printed.contains("zq9"),
            "the unclosed-placeholder error leaked the secret around it"
        );
    }

    let error = refused("malformed-secret-bad-name");
    let ConfigError::MalformedPlaceholder { fragment, .. } = &error else {
        panic!("expected MalformedPlaceholder, got another error");
    };
    assert_eq!(fragment, "${bad name}");
    for printed in [error.to_string(), format!("{error:?}")] {
        assert!(
            !printed.contains("zq9")
                && !printed.contains("leading")
                && !printed.contains("trailing"),
            "the bad-name error leaked the secret around the placeholder"
        );
    }
}

#[test]
fn a_secret_yaml_reads_as_a_number_is_refused_without_echoing_it() {
    let error = refused("numeric-secret");
    assert_eq!(
        error,
        ConfigError::InvalidValue {
            key: "providers[code=orange_cm].credentials.client_secret".to_owned(),
            reason: "a secret must be a quoted YAML string (YAML read it as a number or a boolean)"
                .to_owned(),
        }
    );
    for printed in [error.to_string(), format!("{error:?}")] {
        assert!(
            !printed.contains("4242"),
            "the numeric-secret error leaked the secret"
        );
    }
}

#[test]
fn a_set_but_empty_variable_resolves_to_an_empty_string_not_an_error() {
    // The pepper's length is `HashPepper`'s rule in the gateway; the loader's
    // is only that a name the environment sets is not "unresolved".
    let env = env_with(DEFAULT_PROFILE, &[("SMS_HASH_PEPPER", "")]);
    let config = load_in(DEFAULT_PROFILE, &env).unwrap();
    assert!(config.security().unwrap().hash_pepper.is_blank());
}

// --- profiles ----------------------------------------------------------------

#[test]
fn a_profile_with_no_file_is_refused_naming_the_file_it_expected() {
    let error = load_in("prodd", &env_for(DEFAULT_PROFILE)).unwrap_err();
    let ConfigError::UnknownProfile { profile, expected } = &error else {
        panic!("expected UnknownProfile, got {error:?}");
    };
    assert_eq!(profile, "prodd");
    assert!(expected.ends_with("base-prodd.yml"), "{expected}");
    let message = error.to_string();
    assert!(message.contains("\"prodd\""), "{message}");
    assert!(message.contains("base-prodd.yml"), "{message}");
    assert!(message.contains("SMS_PROFILE"), "{message}");
}

#[test]
fn the_default_profile_may_run_on_the_base_alone() {
    // `dev` is what a process loads when nothing names a profile, and the
    // fixtures directory has no `base-dev.yml`.
    load(DEFAULT_PROFILE).expect("the default profile needs no overlay file");
}

// --- the worker does not load the pepper -------------------------------------

fn load_for(
    binary: Binary,
    profile: &str,
    env: &BTreeMap<String, String>,
) -> Result<Config, ConfigError> {
    Config::load_with_env(Some(&base()), profile, binary, &|name| {
        env.get(name).cloned()
    })
}

#[test]
fn the_worker_loads_with_the_pepper_unset_and_the_gateway_refuses() {
    let mut env = env_for("valid-all-providers");
    assert!(env.remove("SMS_HASH_PEPPER").is_some(), "the base reads it");

    let worker = load_for(Binary::Worker, "valid-all-providers", &env)
        .expect("the worker never reads security.*");
    assert!(worker.security.is_none());
    assert_eq!(worker.security(), Err(ConfigError::SecurityNotLoaded));
    assert!(worker.providers.orange_cm.is_some(), "the rest still loads");

    assert_eq!(
        load_for(Binary::Gateway, "valid-all-providers", &env).unwrap_err(),
        ConfigError::UnresolvedPlaceholder("SMS_HASH_PEPPER".to_owned())
    );
}

#[test]
fn a_worker_whose_environment_still_has_the_pepper_is_no_trouble() {
    // An old compose file or chart still hands it over; the YAML reads it as a
    // placeholder, which is a secret's source and not a leftover.
    let env = env_for("valid-all-providers");
    assert!(env.contains_key("SMS_HASH_PEPPER"));
    let worker = load_for(Binary::Worker, "valid-all-providers", &env).unwrap();
    assert!(worker.security.is_none(), "set or not, the worker drops it");
}

#[test]
fn a_document_without_security_is_refused_by_the_gateway_and_loads_for_the_worker() {
    let text = std::fs::read_to_string(base()).unwrap();
    let without = text.replace("security:\n  hash_pepper: ${SMS_HASH_PEPPER}\n", "");
    assert_ne!(
        text, without,
        "the fixture base no longer has a security block"
    );
    let dir = std::env::temp_dir().join(format!("sms-config-no-security-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("application.yml");
    std::fs::write(&path, without).unwrap();

    let load = |binary| Config::load_with_env(Some(&path), DEFAULT_PROFILE, binary, &|_| None);
    assert_eq!(
        load(Binary::Gateway).unwrap_err(),
        ConfigError::Shape("missing field `security`".to_owned())
    );
    assert!(load(Binary::Worker).is_ok());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_worker_still_refuses_a_retired_variable() {
    for retired in RETIRED {
        if retired.env == "SMS_HASH_PEPPER" {
            continue; // the placeholder form is the exempt one, above
        }
        let env = env_with(DEFAULT_PROFILE, &[(retired.env, "anything")]);
        assert_eq!(
            load_for(Binary::Worker, DEFAULT_PROFILE, &env).unwrap_err(),
            ConfigError::RetiredEnv {
                var: retired.env.to_owned(),
                key: retired.key.to_owned(),
            },
            "{} set in the worker's environment",
            retired.env
        );
    }
}

#[test]
fn the_worker_still_refuses_a_literal_pepper_under_livemode() {
    // The rule reads the file as written, and the file is shared: a literal
    // pepper in a livemode document is a leak whichever binary reads it.
    let env = env_for("livemode-literal-pepper");
    assert!(matches!(
        load_for(Binary::Worker, "livemode-literal-pepper", &env),
        Err(ConfigError::LiteralSecret { ref key }) if key == "security.hash_pepper"
    ));
}

// --- shape rules -----------------------------------------------------------

fn shape_message(profile: &str) -> String {
    match refused(profile) {
        ConfigError::Shape(message) => message,
        other => panic!("{profile}: expected Shape, got {other:?}"),
    }
}

#[test]
fn an_unknown_top_level_key_is_refused_and_named() {
    assert!(shape_message("shape-unknown-key").contains("providerz"));
}

#[test]
fn auth_methods_are_not_a_key_yet() {
    // VSA-1 adds the block. Until then a deployment that writes it believes it
    // narrowed something, so the key is refused rather than dropped.
    assert!(shape_message("shape-auth-not-yet").contains("auth"));
}

#[test]
fn an_unknown_provider_code_lists_the_legal_ones() {
    let message = shape_message("shape-unknown-provider-code");
    assert!(message.contains("smpp_somewhere"), "{message}");
    assert!(
        message.contains("orange_cm") && message.contains("mtn_cm"),
        "{message}"
    );
}

#[test]
fn a_misspelt_key_in_any_block_is_refused_and_named() {
    // One fixture per block, because each block declares its own refusal and a
    // test on the document alone would pass with the others missing.
    for (fixture, misspelt) in [
        ("shape-unknown-key-in-deployment", "livemodee"),
        ("shape-unknown-key-in-security", "hash_peper"),
        ("shape-unknown-key-in-api", "idempotency_ttl"),
        ("shape-unknown-key-in-bucket", "brust"),
        ("shape-unknown-key-in-database", "max_connections"),
    ] {
        let message = shape_message(fixture);
        assert!(message.contains(misspelt), "{fixture}: {message}");
    }
}

#[test]
fn a_misspelt_key_at_every_level_of_a_provider_is_refused_and_named() {
    for (fixture, misspelt) in [
        ("shape-provider-unknown-entry-key", "enabled"),
        ("shape-provider-unknown-host-key", "uri"),
        ("shape-provider-unknown-setting", "tps_ceilling"),
    ] {
        let message = shape_message(fixture);
        assert!(message.contains(misspelt), "{fixture}: {message}");
    }
}

#[test]
fn an_unknown_key_inside_a_provider_is_refused() {
    assert!(shape_message("shape-provider-unknown-key").contains("client_secrett"));
}

#[test]
fn an_unquoted_sender_number_is_a_yaml_integer_and_is_refused() {
    let message = shape_message("shape-sender-number-unquoted");
    assert!(
        message.contains("sender_number") || message.contains("invalid type"),
        "{message}"
    );
}

// --- value rules -----------------------------------------------------------

fn invalid_key(profile: &str) -> String {
    match refused(profile) {
        ConfigError::InvalidValue { key, .. } => key,
        other => panic!("{profile}: expected InvalidValue, got {other:?}"),
    }
}

#[test]
fn a_blank_name_is_refused() {
    assert_eq!(
        refused("empty-name"),
        ConfigError::EmptyField {
            key: "deployment.name".to_owned()
        }
    );
}

#[test]
fn a_pool_of_zero_connections_is_refused() {
    assert_eq!(
        invalid_key("invalid-pool-zero"),
        "database.worker_max_connections"
    );
}

#[test]
fn a_bucket_with_no_burst_is_refused() {
    assert_eq!(
        invalid_key("invalid-burst-zero"),
        "api.source_rate_limit.burst"
    );
}

#[test]
fn a_bucket_that_never_refills_is_refused() {
    assert_eq!(
        invalid_key("invalid-refill-zero"),
        "api.rate_limit.refill_per_second"
    );
}

#[test]
fn an_idempotency_ttl_of_zero_is_refused() {
    assert_eq!(invalid_key("invalid-ttl-zero"), "api.idempotency_ttl_secs");
}

#[test]
fn a_public_base_url_that_is_not_a_url_is_refused() {
    assert_eq!(
        invalid_key("invalid-public-base-url"),
        "deployment.public_base_url"
    );
}

#[test]
fn a_negative_tps_ceiling_is_refused() {
    assert_eq!(
        invalid_key("invalid-tps-negative"),
        "providers[code=mtn_cm].settings.tps_ceiling"
    );
}

#[test]
fn a_negative_price_is_refused() {
    assert_eq!(
        invalid_key("invalid-cost-negative"),
        "providers[code=mtn_cm].settings.cost_per_segment_xaf"
    );
}

#[test]
fn an_unknown_message_type_is_refused() {
    assert_eq!(
        invalid_key("invalid-message-type"),
        "providers[code=aws_sms].settings.message_type"
    );
}

#[test]
fn a_sending_setting_with_no_sending_half_is_refused() {
    assert_eq!(
        invalid_key("aws-sending-setting-without-sending"),
        "providers[code=aws_sms].settings.time_to_live_seconds"
    );
}

// --- livemode --------------------------------------------------------------

#[test]
fn livemode_refuses_a_plaintext_public_base_url() {
    assert_eq!(
        refused("livemode-insecure-base-url"),
        ConfigError::InsecureHost {
            key: "deployment.public_base_url".to_owned(),
            url: "http://sms.example.com".to_owned(),
        }
    );
}

#[test]
fn livemode_refuses_a_stub_public_base_url() {
    assert!(matches!(
        refused("livemode-stub-base-url"),
        ConfigError::StubHostInLivemode { ref key, .. } if key == "deployment.public_base_url"
    ));
}

#[test]
fn livemode_refuses_a_stub_provider_host() {
    assert!(matches!(
        refused("livemode-stub-provider-host"),
        ConfigError::StubHostInLivemode { ref key, .. } if key == "providers[code=orange_cm].host.url"
    ));
}

#[test]
fn livemode_refuses_a_plaintext_provider_host() {
    assert!(matches!(
        refused("livemode-insecure-provider-host"),
        ConfigError::InsecureHost { ref key, .. } if key == "providers[code=mtn_cm].host.url"
    ));
}

#[test]
fn livemode_refuses_a_literal_pepper() {
    assert_eq!(
        refused("livemode-literal-pepper"),
        ConfigError::LiteralSecret {
            key: "security.hash_pepper".to_owned()
        }
    );
}

#[test]
fn livemode_refuses_a_literal_client_secret() {
    assert_eq!(
        refused("livemode-literal-client-secret"),
        ConfigError::LiteralSecret {
            key: "providers[code=orange_cm].credentials.client_secret".to_owned()
        }
    );
}

#[test]
fn a_resolved_placeholder_is_not_mistaken_for_a_literal_in_livemode() {
    // `valid-livemode` writes every secret as a placeholder, and the
    // environment resolves each to a literal-looking string. The rule reads the
    // text as written, so that string must not count.
    load("valid-livemode").unwrap();
}

#[test]
fn livemode_refuses_the_fake_sns_certificate_base() {
    assert_eq!(
        refused("livemode-fake-sns-cert-base"),
        ConfigError::FakeSnsCertBaseInLivemode
    );
}

// --- provider rules --------------------------------------------------------

#[test]
fn a_provider_configured_twice_is_refused() {
    assert_eq!(
        refused("duplicate-provider"),
        ConfigError::DuplicateProvider("orange_cm".to_owned())
    );
}

fn missing(profile: &str) -> (String, String) {
    match refused(profile) {
        ConfigError::MissingProviderSetting { provider, key } => (provider, key),
        other => panic!("{profile}: expected MissingProviderSetting, got {other:?}"),
    }
}

#[test]
fn an_orange_entry_without_its_secret_names_it() {
    assert_eq!(
        missing("missing-orange-secret"),
        (
            "orange_cm".to_owned(),
            "credentials.client_secret".to_owned()
        )
    );
}

#[test]
fn a_blank_required_setting_counts_as_missing() {
    assert_eq!(
        missing("blank-orange-client-id"),
        ("orange_cm".to_owned(), "credentials.client_id".to_owned())
    );
}

#[test]
fn an_mtn_entry_without_a_tps_ceiling_is_refused_rather_than_given_one() {
    assert_eq!(
        missing("missing-mtn-tps-ceiling"),
        ("mtn_cm".to_owned(), "settings.tps_ceiling".to_owned())
    );
}

#[test]
fn a_half_filled_aws_sending_half_names_what_is_missing() {
    assert_eq!(
        missing("missing-aws-configuration-set"),
        (
            "aws_sms".to_owned(),
            "settings.configuration_set".to_owned()
        )
    );
}

#[test]
fn an_aws_entry_that_sends_nothing_and_receives_nothing_is_refused() {
    assert_eq!(
        refused("aws-configures-nothing"),
        ConfigError::ProviderConfiguresNothing
    );
}

// --- the cutover -----------------------------------------------------------

#[test]
fn every_retired_variable_is_refused_naming_its_yaml_key() {
    for retired in RETIRED {
        if retired.env == "SMS_HASH_PEPPER" {
            continue; // the base reads it as a placeholder; see the next tests
        }
        let env = env_with(DEFAULT_PROFILE, &[(retired.env, "anything")]);
        // The plain base reads no retired variable as a placeholder.
        let error = load_in(DEFAULT_PROFILE, &env).unwrap_err();
        assert_eq!(
            error,
            ConfigError::RetiredEnv {
                var: retired.env.to_owned(),
                key: retired.key.to_owned(),
            },
            "{} set in the environment",
            retired.env
        );
    }
}

#[test]
fn a_retired_variable_set_to_the_empty_string_is_still_refused() {
    let env = env_with(DEFAULT_PROFILE, &[("MTN_TPS_CEILING", "")]);
    assert!(matches!(
        load_in(DEFAULT_PROFILE, &env),
        Err(ConfigError::RetiredEnv { ref var, .. }) if var == "MTN_TPS_CEILING"
    ));
}

#[test]
fn a_retired_variable_the_yaml_reads_as_a_placeholder_is_a_secrets_source() {
    // SMS_HASH_PEPPER is read by the base as `${SMS_HASH_PEPPER}`, and the
    // overlay reads two Orange variables the same way: all three are set and
    // none is refused.
    let env = env_with(
        "retired-env-read-as-placeholder",
        &[("ORANGE_CM_CLIENT_ID", "id-from-env")],
    );
    let orange = load_in("retired-env-read-as-placeholder", &env)
        .unwrap()
        .providers
        .orange_cm
        .unwrap();
    assert_eq!(orange.client_id, "id-from-env");
}

#[test]
fn a_retired_variable_that_is_unset_is_no_trouble() {
    load(DEFAULT_PROFILE).unwrap();
}

#[test]
fn the_pepper_is_refused_where_the_yaml_does_not_read_it() {
    // `livemode-literal-pepper` writes the pepper inline, so a pepper left in
    // the environment is the old configuration, silently unused.
    let env = env_with(
        "livemode-literal-pepper",
        &[("SMS_HASH_PEPPER", "left-over-from-the-old-flag")],
    );
    assert_eq!(
        load_in("livemode-literal-pepper", &env).unwrap_err(),
        ConfigError::RetiredEnv {
            var: "SMS_HASH_PEPPER".to_owned(),
            key: "security.hash_pepper".to_owned(),
        }
    );
}
