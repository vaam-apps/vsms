//! One test per rule, each against its own fixture.
//!
//! A fixture is `tests/fixtures/base-<name>.yml`, loaded over
//! `tests/fixtures/base.yml` with profile `<name>`, so every case also
//! exercises the overlay merge the real profiles rely on. The environment is
//! injected: `std::env::set_var` is `unsafe` and this workspace forbids it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sms_config::{AuthMethod, Binary, Config, ConfigError, DEFAULT_PROFILE, RETIRED};

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
fn an_unset_whole_placeholder_is_fatal_and_names_its_variable() {
    // The variable's name is not the secret, and it is what the operator needs.
    assert_eq!(
        refused("unresolved-placeholder"),
        ConfigError::UnresolvedPlaceholder("A_VARIABLE_NOBODY_SETS".to_owned())
    );
}

#[test]
fn an_unclosed_placeholder_in_a_secret_is_refused_by_key() {
    assert_eq!(
        refused("malformed-placeholder"),
        ConfigError::SecretNotAPlaceholder {
            key: "security.hash_pepper".to_owned()
        }
    );
}

/// What an error may print is the key and nothing of the value: `Display` and
/// `Debug` are both log lines, and each part of the value is checked
/// separately so that a leak of any one fails by name.
fn assert_value_absent(error: &ConfigError, parts: &[&str]) {
    for (how, printed) in [
        ("Display", error.to_string()),
        ("Debug", format!("{error:?}")),
    ] {
        for part in parts {
            assert!(
                !printed.contains(part),
                "{how} of {error:?} repeats {part:?} from the secret's value"
            );
        }
    }
}

#[test]
fn a_secret_with_a_malformed_placeholder_is_named_by_key_and_never_echoed() {
    // A sandbox may carry a literal secret, and a literal secret may contain
    // `${`. The error is a log line: it gets the key, no fragment, no name.
    let error = refused("malformed-secret-unclosed");
    assert_eq!(
        error,
        ConfigError::SecretNotAPlaceholder {
            key: "providers[code=orange_cm].credentials.client_secret".to_owned()
        }
    );
    assert_value_absent(&error, &["zq9", "not-a-real-secret", "unclosed"]);

    let error = refused("malformed-secret-bad-name");
    assert_eq!(
        error,
        ConfigError::SecretNotAPlaceholder {
            key: "providers[code=orange_cm].credentials.client_secret".to_owned()
        }
    );
    assert_value_absent(&error, &["zq9", "leading", "trailing", "bad name"]);
}

#[test]
fn a_livemode_literal_secret_with_an_unset_placeholder_leaks_neither_part() {
    // Resolve runs before the literal-secret refusal, so a livemode literal that
    // happens to hold `${NAME}` used to be echoed as "references ${NAME}".
    let error = refused("secret-embedded-unresolved");
    assert_eq!(
        error,
        ConfigError::SecretNotAPlaceholder {
            key: "providers[code=orange_cm].credentials.client_secret".to_owned()
        }
    );
    assert_value_absent(
        &error,
        &["zq9", "leading", "trailing", "A_VARIABLE_NOBODY_SETS"],
    );
}

const ORANGE_SECRET: &str = "providers[code=orange_cm].credentials.client_secret";

#[test]
fn a_secret_pasted_over_a_variable_name_is_a_literal_and_never_printed() {
    // `${s3cr3tAlnumValue9}` is a well-formed placeholder to the resolver, and
    // `env_for` even sets that name. Nobody names a variable that way: it is the
    // secret, written where a name goes, and must neither resolve nor be printed
    // as "references ${s3cr3tAlnumValue9}".
    for profile in [
        "secret-pasted-over-variable-name",
        "livemode-secret-pasted-over-variable-name",
    ] {
        assert!(
            env_for(profile).contains_key("s3cr3tAlnumValue9"),
            "{profile}: the environment must offer the name, or this proves nothing"
        );
        let error = refused(profile);
        assert_eq!(
            error,
            ConfigError::SecretNotAPlaceholder {
                key: ORANGE_SECRET.to_owned()
            },
            "{profile}"
        );
        assert_value_absent(&error, &["s3cr3t", "AlnumValue9"]);
    }
}

#[test]
fn a_secret_written_as_an_array_or_a_map_leaks_no_fragment() {
    // The key path of the string inside is `...client_secret[0]` or
    // `...client_secret.part`, which is not the secret's own key.
    for (profile, key) in [
        ("secret-written-as-array", format!("{ORANGE_SECRET}[0]")),
        ("secret-written-as-map", format!("{ORANGE_SECRET}.part")),
    ] {
        let error = refused(profile);
        assert_eq!(
            error,
            ConfigError::SecretNotAPlaceholder { key },
            "{profile}"
        );
        assert_value_absent(&error, &["s3cr", "et-x9"]);
    }
}

#[test]
fn a_malformed_placeholder_outside_a_secret_keeps_its_fragment_and_a_consistent_key() {
    let error = refused("malformed-setting-bad-name");
    assert_eq!(
        error,
        ConfigError::MalformedPlaceholder {
            key: "providers[code=orange_cm].settings.sender_number".to_owned(),
            fragment: "${bad name}".to_owned(),
        }
    );
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
fn only_the_gateway_needs_the_pepper() {
    let mut env = env_for("valid-all-providers");
    assert!(env.remove("SMS_HASH_PEPPER").is_some(), "the base reads it");

    // The worker hashes nothing, and neither does `mtn-subscribe-dlr`.
    for binary in [Binary::Worker, Binary::ProviderTool, Binary::Operator] {
        let loaded = load_for(binary, "valid-all-providers", &env)
            .unwrap_or_else(|error| panic!("{binary:?} never reads security.*: {error}"));
        assert!(loaded.security.is_none(), "{binary:?}");
        assert_eq!(loaded.security(), Err(ConfigError::SecurityNotLoaded));
        assert!(loaded.providers.orange_cm.is_some(), "the rest still loads");
    }

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
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sms-config-no-security");
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
fn the_worker_and_the_provider_tool_still_refuse_a_retired_variable() {
    for binary in [Binary::Worker, Binary::ProviderTool, Binary::Operator] {
        for retired in RETIRED {
            if retired.env == "SMS_HASH_PEPPER" {
                continue; // the placeholder form is the exempt one, above
            }
            let env = env_with(DEFAULT_PROFILE, &[(retired.env, "anything")]);
            assert_eq!(
                load_for(binary, DEFAULT_PROFILE, &env).unwrap_err(),
                ConfigError::RetiredEnv {
                    var: retired.env.to_owned(),
                    key: retired.key.to_owned(),
                },
                "{} set in the environment of {binary:?}",
                retired.env
            );
        }
    }
}

#[test]
fn the_worker_and_the_provider_tool_still_refuse_a_literal_pepper_under_livemode() {
    // The rule reads the file as written, and the file is shared: a literal
    // pepper in a livemode document is a leak whichever binary reads it.
    let env = env_for("livemode-literal-pepper");
    for binary in [Binary::Worker, Binary::ProviderTool, Binary::Operator] {
        assert!(
            matches!(
                load_for(binary, "livemode-literal-pepper", &env),
                Err(ConfigError::LiteralSecret { ref key }) if key == "security.hash_pepper"
            ),
            "{binary:?}"
        );
    }
}

// --- auth.methods ----------------------------------------------------------

fn permitted(config: &Config) -> Vec<&'static str> {
    config
        .auth_methods
        .permitted()
        .map(AuthMethod::as_str)
        .collect()
}

fn served(config: &Config) -> Vec<&'static str> {
    config
        .auth_methods
        .served()
        .map(AuthMethod::as_str)
        .collect()
}

#[test]
fn an_absent_key_permits_all_four_and_serves_two_without_the_blocks() {
    let config = load(DEFAULT_PROFILE).unwrap();
    assert_eq!(
        permitted(&config),
        ["keypair", "machine_tokens", "hmac_webhooks", "human_login"]
    );
    assert_eq!(
        served(&config),
        ["keypair", "hmac_webhooks"],
        "machine_tokens needs its op: block and human_login needs its own and op:"
    );
    assert!(!config.auth_methods.is_explicit());
    assert!(config.op.is_none());
    assert!(config.human_login.is_none());
    assert!(!config.auth_methods.op_served());
}

#[test]
fn the_vaam_shape_permits_the_keypair_alone() {
    let config = load("auth-keypair-only").unwrap();
    assert_eq!(permitted(&config), ["keypair"]);
    assert_eq!(served(&config), ["keypair"]);
    assert!(config.auth_methods.is_explicit());
    assert!(!config.auth_methods.permits(AuthMethod::HmacWebhooks));
}

#[test]
fn a_list_may_keep_the_hmac_webhooks_and_its_order_is_irrelevant() {
    for profile in ["auth-keypair-and-hmac", "auth-order-is-irrelevant"] {
        let config = load(profile).unwrap();
        assert_eq!(
            permitted(&config),
            ["keypair", "hmac_webhooks"],
            "{profile}"
        );
        assert_eq!(served(&config), ["keypair", "hmac_webhooks"], "{profile}");
    }
}

#[test]
fn an_empty_list_and_a_null_are_both_refused_and_neither_is_the_default() {
    assert_eq!(refused("auth-empty"), ConfigError::NoAuthMethods);
    assert_eq!(
        refused("auth-null"),
        ConfigError::NoAuthMethods,
        "`methods:` with its items commented out must not read as an absent key"
    );
}

#[test]
fn an_unknown_method_is_refused_and_the_legal_set_is_named() {
    let error = refused("auth-unknown");
    assert_eq!(
        error,
        ConfigError::UnknownAuthMethod("carrier_pigeon".to_owned())
    );
    let message = error.to_string();
    for legal in ["keypair", "machine_tokens", "hmac_webhooks", "human_login"] {
        assert!(message.contains(legal), "{message}");
    }
}

#[test]
fn a_duplicate_method_is_refused() {
    assert_eq!(
        refused("auth-duplicate"),
        ConfigError::DuplicateAuthMethod(AuthMethod::Keypair)
    );
}

#[test]
fn human_login_named_without_its_block_is_refused() {
    assert_eq!(
        refused("auth-human-login"),
        ConfigError::AuthMethodNotConfigured {
            method: AuthMethod::HumanLogin,
            key: "human_login".to_owned(),
        },
        "it used to be AuthMethodUnavailable; the method has code now, and wants its block"
    );
}

fn not_configured(method: AuthMethod) -> ConfigError {
    ConfigError::AuthMethodNotConfigured {
        method,
        key: "op".to_owned(),
    }
}

#[test]
fn machine_tokens_named_without_the_op_block_is_refused() {
    assert_eq!(
        refused("auth-machine-tokens"),
        not_configured(AuthMethod::MachineTokens)
    );
}

/// `[machine_tokens]` alone is a legal list since VSA-3b: the API accepts a
/// bearer, so a gateway that serves only tokens answers a caller who has one.
/// It still needs its `op:` block (rule 6), and without it is refused for that
/// reason and not as a list with no way in.
#[test]
fn machine_tokens_alone_boots_with_its_block_and_is_not_a_no_way_in() {
    let config = load("auth-machine-tokens-alone-with-op").expect("a legal deployment");
    assert_eq!(permitted(&config), ["machine_tokens"]);
    assert_eq!(served(&config), ["machine_tokens"]);
    assert!(config.auth_methods.machine_tokens_served());
    assert!(!config.auth_methods.permits(AuthMethod::Keypair));

    assert_eq!(
        refused("auth-machine-tokens-alone"),
        not_configured(AuthMethod::MachineTokens),
        "without its block it is the missing block that is refused, not NoWayIn"
    );
    let message = ConfigError::NoWayIn.to_string();
    assert!(
        message.contains("Add keypair or machine_tokens") && !message.contains("VSA-3b"),
        "{message}"
    );
}

#[test]
fn the_op_block_without_either_method_is_dormant_configuration_and_is_refused() {
    let expected = ConfigError::AuthMethodDisabled {
        methods: vec![AuthMethod::MachineTokens, AuthMethod::HumanLogin],
        key: "op".to_owned(),
    };
    for profile in ["op-without-the-method", "op-without-the-method-hmac"] {
        assert_eq!(refused(profile), expected, "{profile}");
    }
    let message = expected.to_string();
    assert!(
        message.contains("delete op")
            && message.contains("add machine_tokens or human_login")
            && message.contains("does not permit machine_tokens or human_login"),
        "the message names both ways out and both methods: {message}"
    );
}

#[test]
fn the_absent_default_serves_machine_tokens_exactly_when_op_is_written() {
    let config = load("op-default-methods").unwrap();
    assert!(!config.auth_methods.is_explicit());
    assert!(config.auth_methods.machine_tokens_served());
    assert_eq!(
        served(&config),
        ["keypair", "machine_tokens", "hmac_webhooks"]
    );
    let op = config.op.expect("the block is read");
    assert_eq!(op.issuer, "https://sms.example.com");
    assert_eq!(op.token_rate_limit.burst, 10);
    assert_eq!(op.signing_key.refresh_interval_secs, 60);
    assert_eq!(op.signing_key.rotation_overlap_secs, 1800);
}

// --- human_login: ----------------------------------------------------------

fn not_configured_human(key: &str) -> ConfigError {
    ConfigError::AuthMethodNotConfigured {
        method: AuthMethod::HumanLogin,
        key: key.to_owned(),
    }
}

#[test]
fn the_absent_default_serves_human_login_exactly_when_both_blocks_are_written() {
    let config = load("human-login-default-methods").unwrap();
    assert!(!config.auth_methods.is_explicit());
    assert!(config.auth_methods.human_login_served());
    assert!(config.auth_methods.machine_tokens_served());
    assert_eq!(
        served(&config),
        ["keypair", "machine_tokens", "hmac_webhooks", "human_login"]
    );
    let human = config.human_login.expect("the block is read");
    assert_eq!(human.console_client.client_id, "sms-console");
    assert_eq!(
        human.console_client.redirect_uris,
        ["https://admin.example.com/api/auth/callback"]
    );
    assert_eq!(human.login_rate_limit.per_email.burst, 5);
    assert_eq!(human.login_rate_limit.aggregate.burst, 10);
}

#[test]
fn human_login_alone_beside_the_keypair_mounts_the_provider_without_serving_tokens() {
    // `op:` is refused only when neither method that rides it is permitted.
    let config = load("auth-human-login-with-blocks").expect("a legal deployment");
    assert_eq!(permitted(&config), ["keypair", "human_login"]);
    assert_eq!(served(&config), ["keypair", "human_login"]);
    assert!(config.auth_methods.human_login_served());
    assert!(!config.auth_methods.machine_tokens_served());
    assert!(config.auth_methods.op_served());
}

#[test]
fn a_list_naming_all_four_with_both_blocks_serves_all_four() {
    let config = load("auth-all-four-with-blocks").unwrap();
    assert_eq!(
        served(&config),
        ["keypair", "machine_tokens", "hmac_webhooks", "human_login"]
    );
}

#[test]
fn the_human_login_block_without_op_is_refused_even_under_the_absent_default() {
    // The sign-in rides the provider. The default permits human_login, so
    // without this rule the block would be accepted and serve nothing.
    assert_eq!(
        refused("human-login-without-op"),
        not_configured_human("op")
    );
    assert_eq!(
        refused("auth-human-login-without-op"),
        not_configured_human("op")
    );
}

#[test]
fn the_human_login_block_without_the_method_is_dormant_configuration_and_is_refused() {
    let expected = ConfigError::AuthMethodDisabled {
        methods: vec![AuthMethod::HumanLogin],
        key: "human_login".to_owned(),
    };
    for profile in [
        "human-login-without-the-method",
        "human-login-beside-tokens-without-the-method",
    ] {
        assert_eq!(refused(profile), expected, "{profile}");
    }
    let message = expected.to_string();
    assert!(
        message.contains("delete human_login") && message.contains("add human_login"),
        "{message}"
    );
}

#[test]
fn human_login_is_not_a_way_in_for_an_api_caller() {
    assert_eq!(refused("auth-hmac-and-human-login"), ConfigError::NoWayIn);
}

#[test]
fn a_human_login_that_is_a_null_is_refused_not_read_as_absent() {
    let message = shape_message("human-login-null");
    assert!(
        message.contains("human_login") || message.contains("struct HumanLogin"),
        "{message}"
    );
}

#[test]
fn a_human_login_block_missing_a_key_or_with_an_unknown_one_is_refused_and_named() {
    assert!(shape_message("shape-human-login-missing-keys").contains("login_rate_limit"));
    assert!(shape_message("shape-human-login-unknown-key").contains("sso_group"));
    assert!(shape_message("shape-console-client-unknown-key").contains("secret"));
}

fn invalid_reason(profile: &str, key: &str) -> String {
    match refused(profile) {
        ConfigError::InvalidValue { key: got, reason } if got == key => reason,
        other => panic!("{profile}: expected InvalidValue for {key}, got {other:?}"),
    }
}

#[test]
fn the_console_client_id_is_a_clean_oauth_client_id_of_the_columns_length() {
    let key = "human_login.console_client.client_id";
    assert_eq!(
        refused("human-login-client-id-blank"),
        ConfigError::EmptyField {
            key: key.to_owned()
        }
    );
    for profile in [
        "human-login-client-id-too-short",
        "human-login-client-id-too-long",
    ] {
        assert!(
            invalid_reason(profile, key).contains("8 to 64"),
            "{profile}"
        );
    }
    assert!(invalid_reason("human-login-client-id-whitespace", key).contains("whitespace"));
    // 64 is the column's own ceiling and loads.
    load("human-login-client-id-longest").expect("64 characters is the longest the column holds");
}

#[test]
fn the_redirect_uris_are_a_non_empty_list_of_clean_distinct_urls() {
    let list = "human_login.console_client.redirect_uris";
    assert!(invalid_reason("human-login-no-redirect-uris", list).contains("at least one"));
    let first = format!("{list}[0]");
    assert!(matches!(
        refused("human-login-redirect-not-a-url"),
        ConfigError::InvalidValue { key, .. } if key == first
    ));
    assert!(invalid_reason("human-login-redirect-fragment", &first).contains("fragment"));
    assert!(invalid_reason("human-login-redirect-whitespace", &first).contains("whitespace"));
    assert!(
        invalid_reason("human-login-redirect-duplicate", &format!("{list}[1]")).contains("twice")
    );
}

#[test]
fn both_login_buckets_are_range_checked() {
    for (fixture, key) in [
        (
            "human-login-bucket-burst-zero",
            "human_login.login_rate_limit.per_email.burst",
        ),
        (
            "human-login-bucket-refill-zero",
            "human_login.login_rate_limit.per_email.refill_per_second",
        ),
        (
            "human-login-per-email-refill-negative",
            "human_login.login_rate_limit.per_email.refill_per_second",
        ),
        (
            "human-login-aggregate-burst-zero",
            "human_login.login_rate_limit.aggregate.burst",
        ),
        (
            "human-login-aggregate-refill-zero",
            "human_login.login_rate_limit.aggregate.refill_per_second",
        ),
        (
            "human-login-aggregate-refill-negative",
            "human_login.login_rate_limit.aggregate.refill_per_second",
        ),
        (
            "human-login-aggregate-refill-nan",
            "human_login.login_rate_limit.aggregate.refill_per_second",
        ),
    ] {
        assert!(
            matches!(
                refused(fixture),
                ConfigError::InvalidValue { key: got, .. } if got == key
            ),
            "{fixture} must be refused naming {key}"
        );
    }
}

/// The two buckets are separate keys: the old flat bucket (one rate for the
/// email and the source alike) is a shape error, a bucket nobody asked for is
/// refused rather than ignored, and both are required.
#[test]
fn the_login_rate_limit_names_exactly_its_two_buckets() {
    let flat = shape_message("shape-login-rate-limit-flat");
    assert!(flat.contains("per_email"), "{flat}");
    assert!(shape_message("shape-login-rate-limit-unknown-key").contains("per_source"));
    assert!(shape_message("shape-login-rate-limit-no-aggregate").contains("aggregate"));
}

#[test]
fn under_livemode_the_redirect_uris_are_https_and_not_a_stub() {
    // The second entry is the insecure one: every entry is checked, not the first.
    assert!(matches!(
        refused("human-login-livemode-insecure-redirect"),
        ConfigError::InsecureHost { key, url }
            if key == "human_login.console_client.redirect_uris[1]"
                && url == "http://admin.example.com/cb"
    ));
    assert!(matches!(
        refused("human-login-livemode-stub-redirect"),
        ConfigError::StubHostInLivemode { key, .. }
            if key == "human_login.console_client.redirect_uris[0]"
    ));
    load("human-login-livemode-valid").expect("a real https redirect is fine in livemode");
    load("human-login-sandbox-may-use-http")
        .expect("a sandbox may redirect to localhost over http");
}

#[test]
fn a_list_naming_machine_tokens_with_its_block_serves_it() {
    let config = load("auth-machine-tokens-with-op").unwrap();
    assert_eq!(permitted(&config), ["keypair", "machine_tokens"]);
    assert_eq!(served(&config), ["keypair", "machine_tokens"]);
}

#[test]
fn an_op_that_is_a_null_is_refused_not_read_as_absent() {
    let message = shape_message("op-null");
    assert!(
        message.contains("op") || message.contains("struct Op"),
        "{message}"
    );
}

#[test]
fn an_op_block_missing_a_key_or_with_an_unknown_one_is_refused_and_named() {
    assert!(shape_message("shape-op-missing-keys").contains("token_rate_limit"));
    assert!(shape_message("shape-op-unknown-key").contains("kid"));
}

#[test]
fn the_issuer_is_a_url_without_a_trailing_slash() {
    assert!(matches!(
        refused("op-issuer-not-a-url"),
        ConfigError::InvalidValue { key, .. } if key == "op.issuer"
    ));
    let error = refused("op-issuer-trailing-slash");
    assert!(
        matches!(&error, ConfigError::InvalidValue { key, reason }
            if key == "op.issuer" && reason.contains("slash")),
        "{error:?}"
    );
}

#[test]
fn under_livemode_the_issuer_is_https_and_not_a_stub() {
    assert!(matches!(
        refused("op-livemode-insecure-issuer"),
        ConfigError::InsecureHost { key, .. } if key == "op.issuer"
    ));
    assert!(matches!(
        refused("op-livemode-stub-issuer"),
        ConfigError::StubHostInLivemode { key, .. } if key == "op.issuer"
    ));
    load("op-livemode-valid").expect("a real https issuer is fine in livemode");
}

#[test]
fn the_op_buckets_and_timings_are_range_checked() {
    for (profile, key) in [
        ("op-bucket-burst-zero", "op.token_rate_limit.burst"),
        ("op-refresh-zero", "op.signing_key.refresh_interval_secs"),
        (
            "op-overlap-not-above-token-ttl",
            "op.signing_key.rotation_overlap_secs",
        ),
        // 1800 clears the 900-second lifetime alone and not refresh (3600) + 900:
        // the last token a running serve signs under the old key outlives it.
        (
            "op-overlap-not-above-refresh-plus-token-ttl",
            "op.signing_key.rotation_overlap_secs",
        ),
        // The boundary is exclusive: 60 + 900 = 960 is refused, 961 loads below.
        (
            "op-overlap-at-refresh-plus-token-ttl",
            "op.signing_key.rotation_overlap_secs",
        ),
    ] {
        assert!(
            matches!(refused(profile), ConfigError::InvalidValue { key: got, .. } if got == key),
            "{profile}"
        );
    }
}

#[test]
fn the_overlap_rule_names_the_refresh_interval_and_has_an_exclusive_boundary() {
    let ConfigError::InvalidValue { reason, .. } =
        refused("op-overlap-not-above-refresh-plus-token-ttl")
    else {
        panic!("expected InvalidValue");
    };
    assert!(
        reason.contains("refresh_interval_secs (3600)") && reason.contains("4500"),
        "{reason}"
    );
    let config = load("op-overlap-just-above-refresh-plus-token-ttl")
        .expect("one second above refresh + token lifetime is fine");
    assert_eq!(
        config.op.expect("block").signing_key.rotation_overlap_secs,
        961
    );
}

#[test]
fn a_list_that_authenticates_nobody_is_refused() {
    assert_eq!(refused("auth-hmac-only"), ConfigError::NoWayIn);
}

#[test]
fn every_binary_resolves_the_same_methods() {
    // The worker enforces the hmac gate and the gateway serves the discovery
    // document; both must read one answer from one file.
    let env = env_for("auth-keypair-only");
    for binary in [
        Binary::Gateway,
        Binary::Worker,
        Binary::ProviderTool,
        Binary::Operator,
    ] {
        let config = Config::load_with_env(Some(&base()), "auth-keypair-only", binary, &|name| {
            env.get(name).cloned()
        })
        .unwrap();
        assert_eq!(permitted(&config), ["keypair"], "{binary:?}");
    }
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
fn a_misspelt_auth_key_is_refused_and_named_not_dropped() {
    // vpay needs two rules of its own for these because its document ignores
    // unknown top-level keys. Here every block refuses them, so a narrowing that
    // was written in the wrong place is an error and not an all-four deployment.
    for (profile, key) in [
        ("shape-auth-methods-misspelt", "auth_methods"),
        ("shape-auth-misplaced", "auth"),
        ("shape-auth-unknown-key", "method"),
    ] {
        let message = shape_message(profile);
        assert!(message.contains(key), "{profile}: {message}");
    }
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

// --- human_login.sso (VSA-4c) ------------------------------------------------

const SSO_KEY: &str = "human_login.sso";

fn sso_invalid(profile: &str, key: &str) -> String {
    invalid_reason(profile, &format!("{SSO_KEY}.{key}"))
}

#[test]
fn a_complete_sso_block_loads_and_is_read_whole() {
    let config = load("sso-valid").expect("every key written loads");
    let sso = config.sso().expect("the gateway reads the block");
    assert_eq!(sso.issuer, "https://auth.example.org/realms/staff");
    assert_eq!(sso.client_id, "vsms-gateway");
    assert!(
        sso.client_secret.expose() == "SMS_SSO_CLIENT_SECRET-value",
        "the secret reaches the broker from its placeholder, as written"
    );
    assert_eq!(sso.scopes, ["openid", "email", "profile"]);
    assert_eq!(sso.required_group, "vsms-staff");
    assert_eq!(sso.groups_claim, "groups");
    assert_eq!(sso.accepted_amr, ["otp"]);
    assert_eq!(sso.accepted_acr, Vec::<String>::new());
    assert_eq!(sso.jit_role, "support");
    assert_eq!(sso.jwks_min_refresh_secs, 30);
    assert_eq!(sso.rate_limit.burst, 30);
    // A `Debug` print of the whole configuration never carries the secret.
    assert!(!format!("{config:?}").contains("SMS_SSO_CLIENT_SECRET-value"));
}

#[test]
fn an_absent_sso_block_is_a_password_only_deployment() {
    let config = load("human-login-default-methods").unwrap();
    assert!(config.human_login.is_some() && config.sso().is_none());
}

#[test]
fn an_sso_that_is_a_null_is_refused_not_read_as_absent() {
    let message = shape_message("sso-null");
    assert!(
        message.contains("sso") || message.contains("struct Sso"),
        "{message}"
    );
}

#[test]
fn an_sso_block_missing_a_key_or_with_an_unknown_one_is_refused_and_named() {
    assert!(shape_message("sso-missing-key").contains("jit_role"));
    // The redirect URI used to be a key. It is derived now, so writing it is an
    // error and not a value that is silently never read.
    assert!(shape_message("sso-unknown-redirect-uri").contains("redirect_uri"));
}

#[test]
fn sso_is_a_key_of_human_login_and_nowhere_else() {
    assert!(shape_message("sso-at-top-level").contains("sso"));
}

#[test]
fn sso_follows_the_human_login_block_it_is_nested_in() {
    // Without the method, human_login: is refused and so is what is inside it.
    assert_eq!(
        refused("sso-with-human-login-not-permitted"),
        ConfigError::AuthMethodDisabled {
            methods: vec![AuthMethod::HumanLogin],
            key: "human_login".to_owned(),
        }
    );
    // Without op:, the sign-in has no provider to ride.
    assert_eq!(refused("sso-without-op"), not_configured_human("op"));
}

#[test]
fn the_issuer_is_a_url_with_a_host_and_nothing_else_on_it() {
    assert!(matches!(
        refused("sso-issuer-not-a-url"),
        ConfigError::InvalidValue { key, .. } if key == "human_login.sso.issuer"
    ));
    assert!(sso_invalid("sso-issuer-http-remote", "issuer").contains("https"));
    assert!(sso_invalid("sso-issuer-query", "issuer").contains("query"));
    assert!(sso_invalid("sso-issuer-fragment", "issuer").contains("fragment"));
    assert!(sso_invalid("sso-issuer-userinfo", "issuer").contains("credentials"));
}

#[test]
fn the_issuer_is_kept_byte_for_byte_and_plain_http_is_for_loopback_only() {
    let config = load("sso-issuer-verbatim-trailing-slash").unwrap();
    assert_eq!(
        config.sso().unwrap().issuer,
        "https://auth.example.org/realms/staff/",
        "the trailing slash is part of the issuer the id_token must carry"
    );
    let config = load("sso-loopback-http-issuer").expect("a developer's Keycloak");
    assert_eq!(
        config.sso().unwrap().issuer,
        "http://127.0.0.1:8180/realms/staff"
    );
}

#[test]
fn under_livemode_the_sso_issuer_is_https_and_not_a_stub() {
    load("sso-valid-livemode").expect("a real https issuer is fine in livemode");
    assert!(matches!(
        refused("sso-livemode-stub-issuer"),
        ConfigError::StubHostInLivemode { key, .. } if key == "human_login.sso.issuer"
    ));
    assert!(matches!(
        refused("sso-livemode-http-loopback-issuer"),
        ConfigError::InsecureHost { key, .. } if key == "human_login.sso.issuer"
    ));
}

#[test]
fn the_client_id_is_clean_text() {
    assert_eq!(
        refused("sso-client-id-blank"),
        ConfigError::EmptyField {
            key: "human_login.sso.client_id".to_owned()
        }
    );
    assert!(sso_invalid("sso-client-id-whitespace", "client_id").contains("whitespace"));
}

#[test]
fn the_client_secret_is_a_secret_in_every_way_a_secret_is() {
    // Blank, and a number YAML would normalise, are refused naming the key.
    assert_eq!(
        refused("sso-client-secret-blank"),
        ConfigError::EmptyField {
            key: "human_login.sso.client_secret".to_owned()
        }
    );
    let numeric = sso_invalid("sso-client-secret-numeric", "client_secret");
    assert!(numeric.contains("quoted YAML string"), "{numeric}");
    assert!(!numeric.contains("123456"), "the value is never echoed");
    // Under livemode only a placeholder is a secret.
    assert_eq!(
        refused("sso-livemode-literal-client-secret"),
        ConfigError::LiteralSecret {
            key: "human_login.sso.client_secret".to_owned()
        }
    );
    // A sandbox may write one.
    let config = load("sso-literal-client-secret-sandbox").unwrap();
    assert!(
        config.sso().unwrap().client_secret.expose() == "a-sandbox-literal-secret",
        "a sandbox's literal secret reaches the broker as written"
    );
}

#[test]
fn the_scopes_are_a_list_of_single_distinct_scopes_that_includes_openid() {
    let key = |index: usize| format!("{SSO_KEY}.scopes[{index}]");
    assert!(sso_invalid("sso-scopes-empty", "scopes").contains("at least one"));
    assert!(sso_invalid("sso-scopes-without-openid", "scopes").contains("openid"));
    assert!(invalid_reason("sso-scopes-blank-entry", &key(1)).contains("no whitespace"));
    assert!(invalid_reason("sso-scopes-whitespace-entry", &key(1)).contains("no whitespace"));
    assert!(invalid_reason("sso-scopes-duplicate", &key(2)).contains("twice"));
}

#[test]
fn the_group_gate_names_a_group_and_a_claim() {
    assert_eq!(
        refused("sso-required-group-blank"),
        ConfigError::EmptyField {
            key: "human_login.sso.required_group".to_owned()
        }
    );
    assert_eq!(
        refused("sso-groups-claim-blank"),
        ConfigError::EmptyField {
            key: "human_login.sso.groups_claim".to_owned()
        }
    );
}

#[test]
fn the_multi_factor_rule_is_never_empty_and_either_list_may_carry_it() {
    // Both empty would read as no rule at all: refused.
    assert!(sso_invalid("sso-no-mfa-rule", "accepted_amr").contains("multi-factor rule is empty"));
    // One list is enough, whichever it is.
    let acr_only = load("sso-acr-only").unwrap();
    assert_eq!(acr_only.sso().unwrap().accepted_acr, ["mfa"]);
    assert_eq!(acr_only.sso().unwrap().accepted_amr, Vec::<String>::new());
    // And each entry is compared exactly, so it is written exactly.
    assert!(
        invalid_reason("sso-amr-blank-entry", "human_login.sso.accepted_amr[1]")
            .contains("non-blank")
    );
    assert!(
        invalid_reason("sso-amr-duplicate", "human_login.sso.accepted_amr[1]").contains("twice")
    );
    assert!(
        invalid_reason("sso-acr-padded-entry", "human_login.sso.accepted_acr[0]")
            .contains("whitespace")
    );
}

#[test]
fn a_just_in_time_role_is_a_role_key_and_never_one_that_acts_with_authority() {
    for role in ["owner", "admin", "system", "app"] {
        let reason = sso_invalid(&format!("sso-jit-role-{role}"), "jit_role");
        assert!(
            reason.contains("can never be a just-in-time role"),
            "{role}: {reason}"
        );
    }
    for profile in ["sso-jit-role-uppercase", "sso-jit-role-too-short"] {
        assert!(
            sso_invalid(profile, "jit_role").contains("Role.key"),
            "{profile}"
        );
    }
    // Any other key is the operator's to choose: the role row is checked at boot.
    assert_eq!(
        load("sso-jit-role-operator")
            .unwrap()
            .sso()
            .unwrap()
            .jit_role,
        "operator"
    );
}

#[test]
fn the_refused_roles_are_the_four_a_sign_in_must_never_create() {
    // The list is a copy of sms_api's RESERVED_ROLE_KEYS and DEVICE_ONLY_ROLES,
    // which this crate cannot import. sms-auth pins the copy to them.
    assert_eq!(
        sms_config::REFUSED_JIT_ROLES,
        ["system", "app", "owner", "admin"]
    );
}

#[test]
fn the_jwks_refresh_floor_and_the_rate_limit_are_range_checked() {
    for profile in ["sso-jwks-refresh-zero", "sso-jwks-refresh-too-long"] {
        assert!(
            sso_invalid(profile, "jwks_min_refresh_secs").contains("1 to 3600"),
            "{profile}"
        );
    }
    for (profile, key) in [
        ("sso-rate-limit-burst-zero", "rate_limit.burst"),
        ("sso-rate-limit-refill-zero", "rate_limit.refill_per_second"),
    ] {
        sso_invalid(profile, key);
    }
}

#[test]
fn only_the_gateway_is_handed_the_identity_provider_secret() {
    // The worker's environment has no SMS_SSO_CLIENT_SECRET, and the document
    // still writes ${SMS_SSO_CLIENT_SECRET}: the worker drops the block before it
    // resolves placeholders, so it starts, and is never given the secret.
    let mut env = env_for("sso-valid");
    assert!(env.remove("SMS_SSO_CLIENT_SECRET").is_some());
    for binary in [Binary::Worker, Binary::ProviderTool, Binary::Operator] {
        let loaded = load_for(binary, "sso-valid", &env)
            .unwrap_or_else(|error| panic!("{binary:?} never reads human_login.sso: {error}"));
        assert!(loaded.sso().is_none(), "{binary:?}");
        assert!(
            loaded.human_login.is_some_and(|block| block.sso.is_none()),
            "{binary:?} keeps the rest of the block"
        );
    }
    assert_eq!(
        load_for(Binary::Gateway, "sso-valid", &env).unwrap_err(),
        ConfigError::UnresolvedPlaceholder("SMS_SSO_CLIENT_SECRET".to_owned())
    );
}

#[test]
fn the_worker_still_refuses_a_literal_identity_provider_secret_under_livemode() {
    // The literal check reads the file as written, before the worker drops the
    // block: a leaked secret in a shared document is a leak whichever binary reads it.
    let env = env_for("sso-livemode-literal-client-secret");
    for binary in [Binary::Worker, Binary::ProviderTool, Binary::Operator] {
        assert_eq!(
            load_for(binary, "sso-livemode-literal-client-secret", &env).unwrap_err(),
            ConfigError::LiteralSecret {
                key: "human_login.sso.client_secret".to_owned()
            },
            "{binary:?}"
        );
    }
}

#[test]
fn a_retired_sso_variable_the_yaml_reads_is_the_secrets_source_and_the_rest_are_refused() {
    // SMS_SSO_CLIENT_SECRET is read as a placeholder by sso-valid, so it is the
    // secret's source and not a leftover. Any other SMS_SSO_* variable is the old
    // flag's and names the key that replaced it.
    load("sso-valid").unwrap();
    let env = env_with(
        "sso-valid",
        &[("SMS_SSO_REDIRECT_URI", "https://x.example/cb")],
    );
    assert_eq!(
        load_in("sso-valid", &env).unwrap_err(),
        ConfigError::RetiredEnv {
            var: "SMS_SSO_REDIRECT_URI".to_owned(),
            key: "op.issuer".to_owned(),
        }
    );
    let env = env_with(
        "sso-valid",
        &[("SMS_SSO_ISSUER", "https://auth.example.org")],
    );
    assert!(matches!(
        load_in("sso-valid", &env),
        Err(ConfigError::RetiredEnv { ref key, .. }) if key == "human_login.sso.issuer"
    ));
}
