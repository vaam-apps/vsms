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

/// The variables only the gateway reads: every `${NAME}` under `security:` in
/// the base file. The worker does not load that block (`Binary::Worker`), so a
/// stack that hands it one gives a process a secret it never uses.
fn gateway_only_variables() -> BTreeSet<String> {
    fn collect(value: &Value, out: &mut BTreeSet<String>) {
        match value {
            Value::String(text) => {
                let mut rest = text.as_str();
                while let Some(start) = rest.find("${") {
                    let after = &rest[start + 2..];
                    let Some(end) = after.find('}') else { break };
                    out.insert(after[..end].to_owned());
                    rest = &after[end + 1..];
                }
            }
            Value::Mapping(map) => map.values().for_each(|child| collect(child, out)),
            Value::Sequence(items) => items.iter().for_each(|child| collect(child, out)),
            _ => {}
        }
    }
    let mut names = BTreeSet::new();
    collect(&yaml("config/application.yml")["security"], &mut names);
    assert!(
        names.contains("SMS_HASH_PEPPER"),
        "the base file's security block no longer reads the pepper: {names:?}"
    );
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
        let gateway_only = gateway_only_variables();
        for service in ["sms-gateway", "sms-worker"] {
            let env = compose_env(&compose, service);
            for name in &wanted {
                let wanted_here = service == "sms-gateway" || !gateway_only.contains(name);
                assert_eq!(
                    env.contains(name),
                    wanted_here,
                    "{file}: {service} {} {name}, which config/application*.yml reads as \
                     ${{{name}}} under profile {profile} (security.* is the gateway's alone)",
                    if wanted_here {
                        "does not forward"
                    } else {
                        "is given"
                    },
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
    assert!(
        config.get("createOverlayConfigMap").is_none(),
        "values.yaml still has a switch for the overlay ConfigMap, which is never optional"
    );
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
        "SMS_SSO_CLIENT_SECRET",
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
        for needed in ["SMS_PROFILE", "SMS_CONFIG"] {
            assert!(
                names.contains(needed),
                "the chart's {controller} lacks {needed}"
            );
        }
        // The pepper is the gateway's alone: the worker does not load `security`.
        assert_eq!(
            names.contains("SMS_HASH_PEPPER"),
            controller == "gateway",
            "the chart's {controller} and the hash pepper"
        );
        // So is the identity-provider secret (VSA-4c): the worker drops
        // `human_login.sso` before it resolves placeholders.
        assert_eq!(
            names.contains("SMS_SSO_CLIENT_SECRET"),
            controller == "gateway",
            "the chart's {controller} and the identity-provider secret"
        );
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

/// The gateway's log filter names `sms_auth` (VSA-4c). The crate logs every refusal
/// of a sign-in (`email_in_use`, a login the Argon2 check refused, a console
/// client that drifted) and an unnamed target is `ERROR`-only under a filter that
/// lists the others: VS-4 dropped it when it deleted the code that needed it, and
/// the sign-in's errors went missing from the log until now. Three places spell the
/// default - the binary, the chart, the compose stack - and they must agree.
#[test]
fn the_gateways_default_log_filter_names_sms_auth_everywhere_it_is_written() {
    let main =
        std::fs::read_to_string(repo().join("backends/apps/sms-gateway/src/main.rs")).unwrap();
    assert!(
        main.contains("sms_gateway=info,sms_api=info,sms_auth=info,"),
        "main.rs's default filter lost sms_auth"
    );
    let values = yaml("deploy/charts/vsms/values.yaml");
    let chart = values["controllers"]["gateway"]["containers"]["main"]["env"]["RUST_LOG"]["value"]
        .as_str()
        .expect("the chart sets the gateway's RUST_LOG");
    assert!(
        chart.contains("sms_auth=info"),
        "the chart's filter: {chart}"
    );
    let compose = yaml("deploy/docker-compose.yml");
    let filter = compose["services"]["sms-gateway"]["environment"]["RUST_LOG"]
        .as_str()
        .expect("compose sets the gateway's RUST_LOG");
    assert!(
        filter.contains("sms_auth=info"),
        "compose's filter: {filter}"
    );
    let example = std::fs::read_to_string(repo().join("deploy/.env.example")).unwrap();
    assert!(
        example.contains("SMS_GATEWAY_LOG=sms_gateway=info,sms_api=info,sms_auth=info,"),
        ".env.example's documented default lost sms_auth"
    );
}

/// The secret's commented line in the production compose file is the bare-key
/// form. A key with a value, or `${...:-}`, is present in the environment when
/// uncommented, and a variable the overlay does not read is refused at boot - so
/// the form is what makes uncommenting it safe only when the overlay reads it.
#[test]
fn the_compose_file_offers_the_sso_secret_as_a_bare_key_for_the_gateway_only() {
    let text = std::fs::read_to_string(repo().join("deploy/docker-compose.yml")).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| {
            line.trim_start_matches([' ', '#'])
                .starts_with("SMS_SSO_CLIENT_SECRET:")
        })
        .collect();
    assert_eq!(lines, ["      # SMS_SSO_CLIENT_SECRET:"], "{lines:?}");
    let compose = yaml("deploy/docker-compose.yml");
    assert!(
        !compose_env(&compose, "sms-worker").contains("SMS_SSO_CLIENT_SECRET"),
        "the worker is never given the identity-provider secret"
    );
    assert!(
        !compose_env(&compose, "sms-gateway").contains("SMS_SSO_CLIENT_SECRET"),
        "it is offered commented out: the default stack serves no SSO, and a variable the overlay does not read is refused"
    );
}
