//! The VSC-1 hard cutover, against the real `sms-worker` binary.
//!
//! The worker reads the same document as the gateway, so it refuses the same
//! retired names. Nothing here needs Postgres: the configuration is read before
//! the pool is opened, and the database URL below is refused at once.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/application.yml")
}

fn worker(extra: &[&str], env: &[(&str, &str)]) -> Output {
    worker_on("demo", extra, env)
}

fn worker_on(profile: &str, extra: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sms-worker"));
    command
        .args(["--roles", "dispatch"])
        // Not a URL at all: sqlx refuses it at once. A well-formed URL to a closed
        // port would be retried for its whole 30 second acquire timeout.
        .args(["--database-url", "not a database url"])
        .args(["--metrics-listen", "127.0.0.1:0"])
        .arg("--config")
        .arg(base())
        .args(["--profile", profile])
        .args(extra)
        .env_clear()
        .env(
            "SMS_HASH_PEPPER",
            "worker-cutover-pepper-over-the-minimum-length",
        );
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("running sms-worker")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_retired_flag_is_refused_naming_the_yaml_key_that_replaced_it() {
    let output = worker(&["--mtn-tps-ceiling", "5"], &[]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(
        text.contains("--mtn-tps-ceiling no longer exists"),
        "{text}"
    );
    assert!(
        text.contains("providers[code=mtn_cm].settings.tps_ceiling"),
        "{text}"
    );
}

#[test]
fn the_pool_size_flag_is_refused_too() {
    let output = worker(&["--db-max-connections=5"], &[]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("database.worker_max_connections"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_retired_variable_is_refused_naming_the_yaml_key_that_replaced_it() {
    let output = worker(&[], &[("AWS_SMS_REGION", "eu-west-1")]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("AWS_SMS_REGION"), "{text}");
    assert!(
        text.contains("providers[code=aws_sms].settings.region"),
        "{text}"
    );
}

#[test]
fn a_loadable_configuration_gets_past_the_loader_to_the_database() {
    // The control: the `demo` profile configures Orange, so `dispatch` has a
    // provider, and the process goes on to connect to the port nothing listens
    // on. Failing at the loader instead would make the refusals above vacuous.
    let output = worker(&[], &[]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(!text.contains("loading the YAML configuration"), "{text}");
    assert!(text.contains("connecting to Postgres"), "{text}");
}

#[test]
fn dispatch_without_any_provider_names_the_yaml_to_edit() {
    // `application.yml` alone configures no provider (`--profile` has no
    // overlay file of that name).
    let output = worker_on("no-such-profile", &[], &[]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("`providers` entry"),
        "{}",
        stderr(&output)
    );
}
