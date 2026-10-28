//! The VSC-1 hard cutover, against the real binary.
//!
//! `sms-config` proves each refusal on its own; this proves the binary calls
//! it, in the right order. `serve` reads its configuration before it touches a
//! key file or the database, so none of these needs Postgres: every case is
//! refused (or fails further on) before a connection is attempted, and the
//! database URL below points at a port nothing listens on.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PEPPER: &str = "config-cutover-test-pepper-comfortably-over-the-minimum";

fn base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/application.yml")
}

fn serve(extra: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
    command
        .arg("serve")
        .arg("--config")
        .arg(base())
        .args(["--profile", "demo"])
        // clap requires these before main's body runs; the files are never read
        // when the configuration is refused first.
        .args(["--envelope-signing-key-file", "/nonexistent/current.pem"])
        .args(["--envelope-next-signing-key-file", "/nonexistent/next.pem"])
        .args(["--database-url", "postgres://x:x@127.0.0.1:1/x"])
        .args(extra)
        .env_clear()
        .env("SMS_HASH_PEPPER", PEPPER);
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("running sms-gateway serve")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_retired_flag_is_refused_naming_the_yaml_key_that_replaced_it() {
    let output = serve(&["--orange-client-id", "x"], &[]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(
        text.contains("--orange-client-id no longer exists"),
        "{text}"
    );
    assert!(
        text.contains("providers[code=orange_cm].credentials.client_id"),
        "{text}"
    );
}

#[test]
fn a_retired_variable_is_refused_naming_the_yaml_key_that_replaced_it() {
    let output = serve(&[], &[("ORANGE_CM_CLIENT_ID", "left-over")]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("ORANGE_CM_CLIENT_ID"), "{text}");
    assert!(
        text.contains("providers[code=orange_cm].credentials.client_id"),
        "{text}"
    );
}

#[test]
fn every_retired_flag_is_refused_by_serve_and_by_mtn_subscribe_dlr() {
    for retired in sms_config::RETIRED {
        for subcommand in ["serve", "mtn-subscribe-dlr"] {
            let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
                .arg(subcommand)
                .args([retired.flag, "x"])
                .env_clear()
                .output()
                .expect("running sms-gateway");
            let text = stderr(&output);
            assert!(!output.status.success(), "{subcommand} {}", retired.flag);
            assert!(
                text.contains(retired.key),
                "`{subcommand} {}` did not name {}: {text}",
                retired.flag,
                retired.key
            );
        }
    }
}

#[test]
fn an_unset_placeholder_stops_the_process_before_anything_connects() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sms-gateway"));
    let output = command
        .arg("serve")
        .arg("--config")
        .arg(base())
        .args(["--profile", "demo"])
        .args(["--envelope-signing-key-file", "/nonexistent/current.pem"])
        .args(["--envelope-next-signing-key-file", "/nonexistent/next.pem"])
        .args(["--database-url", "postgres://x:x@127.0.0.1:1/x"])
        .env_clear()
        .output()
        .expect("running sms-gateway serve");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("SMS_HASH_PEPPER"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn no_config_path_is_refused_rather_than_guessed() {
    let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .arg("serve")
        .args(["--envelope-signing-key-file", "/nonexistent/current.pem"])
        .args(["--envelope-next-signing-key-file", "/nonexistent/next.pem"])
        .args(["--database-url", "postgres://x:x@127.0.0.1:1/x"])
        .env_clear()
        .output()
        .expect("running sms-gateway serve");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no config path given"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_loadable_configuration_gets_past_the_loader_to_the_keys() {
    // The control for the refusals above: with nothing retired and the pepper
    // set, the process gets further than the loader - as far as the key files,
    // which do not exist. If this ever fails at the loader, the refusals above
    // are passing for the wrong reason.
    let output = serve(&[], &[]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(!text.contains("loading the YAML configuration"), "{text}");
    assert!(text.contains("current.pem"), "{text}");
}

#[test]
fn mtn_subscribe_dlr_without_an_mtn_entry_says_so() {
    let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .arg("mtn-subscribe-dlr")
        .arg("--config")
        .arg(base())
        .args(["--profile", "demo"])
        .args(["--delivery-report-url", "https://sms.example.cm/dlr/mtn_cm"])
        .env_clear()
        .env("SMS_HASH_PEPPER", PEPPER)
        .output()
        .expect("running sms-gateway mtn-subscribe-dlr");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("code: mtn_cm"),
        "{}",
        stderr(&output)
    );
}
