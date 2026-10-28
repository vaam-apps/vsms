//! The deployment files agree with the configuration they feed.
//!
//! The YAML reads `${NAME}` placeholders; the compose files and the Helm chart
//! are what put those names into a container's environment. Nothing else
//! connects the two lists, and a name missing from a compose file is a process
//! that refuses to start (correctly, naming it) on the first `up`. These tests
//! read the files as data, so the drift is a failing test rather than a
//! crash-looping container.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;
use sms_config::RETIRED;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn yaml(path: &str) -> Value {
    let text = std::fs::read_to_string(repo().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_yaml_ng::from_str(&text).unwrap_or_else(|e| panic!("{path} is not YAML: {e}"))
}

/// Every `${NAME}` a committed config file writes outside a comment.
fn placeholders(files: &[&str]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(repo().join(file)).unwrap();
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            let mut rest = line;
            while let Some(start) = rest.find("${") {
                let after = &rest[start + 2..];
                let Some(end) = after.find('}') else { break };
                names.insert(after[..end].to_owned());
                rest = &after[end + 1..];
            }
        }
    }
    names
}

fn compose_env(compose: &Value, service: &str) -> BTreeSet<String> {
    compose["services"][service]["environment"]
        .as_mapping()
        .unwrap_or_else(|| panic!("{service} has no environment mapping"))
        .keys()
        .filter_map(|key| key.as_str().map(str::to_owned))
        .collect()
}

#[test]
fn every_compose_stack_forwards_the_variables_its_profile_reads() {
    for (file, profile) in [
        ("compose.dev.yaml", "dev"),
        ("compose.demo.yaml", "demo"),
        ("deploy/docker-compose.yml", "prod"),
    ] {
        let wanted = placeholders(&[
            "config/application.yml",
            &format!("config/application-{profile}.yml"),
        ]);
        let compose = yaml(file);
        for service in ["sms-gateway", "sms-worker"] {
            let env = compose_env(&compose, service);
            for name in &wanted {
                assert!(
                    env.contains(name),
                    "{file}: {service} does not forward {name}, which config/application*.yml \
                     reads as ${{{name}}} under profile {profile}"
                );
            }
            assert_eq!(
                compose["services"][service]["environment"]["SMS_PROFILE"]
                    .as_str()
                    .map(|p| p.replace("${SMS_PROFILE:-prod}", "prod")),
                Some(profile.to_owned()),
                "{file}: {service} loads the wrong profile"
            );
        }
    }
}

#[test]
fn no_compose_stack_sets_a_retired_variable_the_profile_does_not_read() {
    for (file, profile) in [
        ("compose.dev.yaml", "dev"),
        ("compose.demo.yaml", "demo"),
        ("deploy/docker-compose.yml", "prod"),
    ] {
        let read = placeholders(&[
            "config/application.yml",
            &format!("config/application-{profile}.yml"),
        ]);
        let compose = yaml(file);
        for service in ["sms-gateway", "sms-worker"] {
            for name in compose_env(&compose, service) {
                let retired = RETIRED.iter().any(|retired| retired.env == name);
                assert!(
                    !retired || read.contains(&name),
                    "{file}: {service} sets {name}, retired by VSC-1, and profile {profile} does \
                     not read it - the process would refuse to start"
                );
            }
        }
    }
}

#[test]
fn the_production_stack_mounts_the_overlay_it_loads() {
    let compose = yaml("deploy/docker-compose.yml");
    assert_eq!(
        compose["configs"]["sms_config_overlay"]["file"].as_str(),
        Some("../config/application-prod.yml")
    );
    for service in ["sms-gateway", "sms-worker"] {
        let mounts = compose["services"][service]["configs"]
            .as_sequence()
            .unwrap_or_else(|| panic!("{service} mounts no config"));
        assert!(
            mounts.iter().any(|mount| {
                mount["source"].as_str() == Some("sms_config_overlay")
                    && mount["target"].as_str() == Some("/config/application-prod.yml")
            }),
            "{service} does not mount the prod overlay"
        );
    }
}

#[test]
fn the_chart_no_longer_carries_the_values_the_yaml_replaced() {
    let values = yaml("deploy/charts/vsms/values.yaml");
    for retired in ["orange", "mtn", "awsSms"] {
        assert!(
            values.get(retired).is_none(),
            "values.yaml still defines `{retired}`, which VSC-1 removed"
        );
    }
    let config = &values["config"];
    assert_eq!(config["createOverlayConfigMap"].as_bool(), Some(false));
    assert_eq!(config["path"].as_str(), Some("/config/application.yml"));
    assert!(config["overlay"].as_str().is_some());
}

#[test]
fn the_chart_declares_no_retired_variable_but_the_secrets_the_overlay_reads() {
    // The three provider secrets and the pepper are retired as FLAGS and stay as
    // `${VAR}` sources; any other retired name in a controller's env is a value
    // the process would refuse.
    let sources: BTreeSet<&str> = [
        "SMS_HASH_PEPPER",
        "ORANGE_CM_CLIENT_SECRET",
        "MTN_CLIENT_SECRET",
        "AWS_SMS_SECRET_ACCESS_KEY",
    ]
    .into();
    let values = yaml("deploy/charts/vsms/values.yaml");
    for controller in ["gateway", "worker"] {
        let env = values["controllers"][controller]["containers"]["main"]["env"]
            .as_mapping()
            .unwrap_or_else(|| panic!("{controller} has no env"));
        let names: BTreeSet<&str> = env.keys().filter_map(Value::as_str).collect();
        for retired in RETIRED {
            assert!(
                !names.contains(retired.env) || sources.contains(retired.env),
                "the chart's {controller} sets {}, retired by VSC-1",
                retired.env
            );
        }
        for needed in ["SMS_PROFILE", "SMS_CONFIG", "SMS_HASH_PEPPER"] {
            assert!(
                names.contains(needed),
                "the chart's {controller} lacks {needed}"
            );
        }
    }
}

#[test]
fn the_dockerfiles_bake_the_configuration_the_chart_overlays() {
    for dockerfile in [
        "backends/apps/sms-gateway/Dockerfile",
        "backends/apps/sms-worker/Dockerfile",
    ] {
        let text = std::fs::read_to_string(repo().join(dockerfile)).unwrap();
        assert!(
            text.lines()
                .any(|l| l == "COPY --from=builder /app/config /config"),
            "{dockerfile} does not bake config/ at /config"
        );
        assert!(
            text.lines()
                .any(|l| l == "ENV SMS_CONFIG=/config/application.yml"),
            "{dockerfile} does not name the base file"
        );
    }
}
