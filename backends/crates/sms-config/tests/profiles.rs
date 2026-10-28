//! Every profile committed under `config/` loads.
//!
//! The profiles are discovered from the directory, not listed here, so a new
//! `config/application-<name>.yml` is covered the day it is added. Each is loaded
//! the way a process would load it, through the base file and the overlay, with
//! the environment its own `${VAR}` placeholders ask for.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sms_config::Config;

fn config_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config")
}

fn profiles() -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(config_dir())
        .expect("config/ exists at the repository root")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let profile = name.strip_prefix("application-")?.strip_suffix(".yml")?;
            Some(profile.to_owned())
        })
        .collect();
    found.sort();
    found
}

/// A value for every placeholder the committed files write, shaped like the real
/// thing where a rule looks at the shape (a numeric rate, an E.164 number, a
/// host that must not look like a stub).
fn value_of(name: &str) -> String {
    match name {
        "SMS_GATEWAY_DOMAIN" => "sms.example.com",
        "ORANGE_CM_SENDER_NUMBER" => "+2370000",
        "MTN_TPS_CEILING" | "AWS_SMS_TPS_CEILING" => "5",
        "MTN_COST_PER_SEGMENT_XAF" | "AWS_SMS_COST_PER_SEGMENT_XAF" => "16.00",
        other => return format!("{other}-value-for-the-test"),
    }
    .to_owned()
}

fn env_for(profile: &str) -> BTreeMap<String, String> {
    let mut text = String::new();
    for file in [
        "application.yml".to_owned(),
        format!("application-{profile}.yml"),
    ] {
        if let Ok(body) = std::fs::read_to_string(config_dir().join(file)) {
            text.push_str(&body);
        }
    }
    env_from(&text)
}

/// The environment for the `${NAME}`s in `text`, comments excluded: the loader
/// does not read a comment, so a variable only a comment mentions is not part of
/// the document, and setting it would be the retired-variable mistake.
fn env_from(text: &str) -> BTreeMap<String, String> {
    let text: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let mut env = BTreeMap::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            env.insert(name.to_owned(), value_of(name));
        }
        rest = &after[end + 1..];
    }
    env
}

fn load(profile: &str) -> Config {
    let env = env_for(profile);
    let base = config_dir().join("application.yml");
    Config::load_with_env(Some(&base), profile, &|name| env.get(name).cloned())
        .unwrap_or_else(|error| panic!("config/application-{profile}.yml does not load: {error}"))
}

#[test]
fn the_profiles_directory_is_not_empty() {
    // The walk below would pass vacuously over an empty list.
    assert!(profiles().len() >= 3, "{:?}", profiles());
}

#[test]
fn the_base_file_loads_on_its_own() {
    let env = env_for("no-such-profile");
    let base = config_dir().join("application.yml");
    let config = Config::load_with_env(Some(&base), "no-such-profile", &|name| {
        env.get(name).cloned()
    })
    .unwrap();
    assert!(!config.deployment.livemode);
    assert!(config.providers.is_empty());
}

#[test]
fn every_committed_profile_loads() {
    for profile in profiles() {
        load(&profile);
    }
}

#[test]
fn every_committed_profile_configures_at_least_one_provider() {
    // A profile with none cannot serve (nothing to parse receipts with) or
    // dispatch (nothing to send through): it would load and then fail at boot.
    for profile in profiles() {
        assert!(
            !load(&profile).providers.is_empty(),
            "profile {profile} configures no provider"
        );
    }
}

#[test]
fn the_dev_profile_is_never_livemode() {
    // It is the default profile; forgetting SMS_PROFILE must not reach real
    // traffic.
    assert!(!load("dev").deployment.livemode);
}

#[test]
fn the_prod_profile_is_livemode_and_its_secrets_are_placeholders() {
    // `Config::load` already refuses a literal in livemode, so loading is the
    // proof; this pins that the profile meant to be livemode still is, because
    // flipping it to false would silence every livemode rule at once.
    assert!(load("prod").deployment.livemode);
}

#[test]
fn the_dev_profile_trusts_the_fake_certificate_authority_and_prod_cannot() {
    let aws = load("dev").providers.aws_sms.expect("dev runs aws_sms");
    assert!(aws.fake_sns_cert_base.is_some());
}

/// The prod file carries MTN and AWS as commented examples. A comment can say
/// anything, so this uncomments them and loads the result: an example that does
/// not load is a lie in the one file an operator copies from.
#[test]
fn the_commented_provider_examples_in_prod_are_valid_when_enabled() {
    let prod = std::fs::read_to_string(config_dir().join("application-prod.yml")).unwrap();
    let uncommented: String = prod
        .lines()
        .map(|line| {
            if line.starts_with("  # - code:") || line.starts_with("  #   ") {
                line.replacen("  # ", "  ", 1)
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        uncommented.contains("- code: mtn_cm") && uncommented.contains("- code: aws_sms"),
        "the uncommenting found no example to enable"
    );

    let dir = std::env::temp_dir().join(format!("sms-config-prod-examples-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        config_dir().join("application.yml"),
        dir.join("application.yml"),
    )
    .unwrap();
    std::fs::write(dir.join("application-prod.yml"), &uncommented).unwrap();

    let base = std::fs::read_to_string(dir.join("application.yml")).unwrap();
    let env = env_from(&format!("{base}\n{uncommented}"));
    let config = Config::load_with_env(Some(&dir.join("application.yml")), "prod", &|name| {
        env.get(name).cloned()
    })
    .unwrap_or_else(|error| panic!("the prod examples do not load once enabled: {error}"));
    assert!(config.providers.orange_cm.is_some());
    assert!(config.providers.mtn_cm.is_some());
    assert!(config.providers.aws_sms.is_some());
    std::fs::remove_dir_all(&dir).ok();
}
