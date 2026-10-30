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
    // No pepper in the environment, on purpose: the command registers a URL with
    // MADAPI and hashes nothing, so it loads as `Binary::ProviderTool` and must
    // get as far as the missing `mtn_cm` entry. Loaded as the gateway, it would
    // stop at `${SMS_HASH_PEPPER}` first.
    let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .arg("mtn-subscribe-dlr")
        .arg("--config")
        .arg(base())
        .args(["--profile", "demo"])
        .args(["--delivery-report-url", "https://sms.example.cm/dlr/mtn_cm"])
        .env_clear()
        .output()
        .expect("running sms-gateway mtn-subscribe-dlr");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("code: mtn_cm"),
        "{}",
        stderr(&output)
    );
}

// --- VSA-3a: auth.methods and the op: block, against the real binary ---------

use sms_test_support::config::{self, TestConfig};

fn overlay_config(tag: &str, overlay: &str) -> TestConfig {
    TestConfig::write(Path::new(env!("CARGO_TARGET_TMPDIR")), tag, overlay)
}

fn serve_with(config: &TestConfig) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .arg("serve")
        .args(config.args())
        .args(["--envelope-signing-key-file", "/nonexistent/current.pem"])
        .args(["--envelope-next-signing-key-file", "/nonexistent/next.pem"])
        .args(["--database-url", "postgres://x:x@127.0.0.1:1/x"])
        .env_clear()
        .env(config::PEPPER_VAR, config::PEPPER)
        .output()
        .expect("running sms-gateway serve")
}

const OP_BLOCK: &str = "op:\n  issuer: https://sms.example.com\n  token_rate_limit:\n    burst: 10\n    \
    refill_per_second: 0.3\n  signing_key:\n    refresh_interval_secs: 60\n    \
    rotation_overlap_secs: 1800\n";

#[test]
fn serve_refuses_machine_tokens_named_without_the_op_block() {
    let config = overlay_config(
        "named-no-op",
        &format!(
            "{}auth:\n  methods: [keypair, machine_tokens]\n",
            config::no_providers_overlay()
        ),
    );
    let output = serve_with(&config);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(
        text.contains("auth.methods names machine_tokens but op is not configured"),
        "{text}"
    );
    assert!(
        !text.contains("current.pem"),
        "refused before the keys: {text}"
    );
}

#[test]
fn serve_refuses_an_op_block_the_methods_do_not_permit() {
    let config = overlay_config(
        "op-no-method",
        &format!(
            "{}auth:\n  methods: [keypair]\n{OP_BLOCK}",
            config::no_providers_overlay()
        ),
    );
    let output = serve_with(&config);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(
        text.contains("op is configured but auth.methods does not permit machine_tokens"),
        "{text}"
    );
    assert!(
        !text.contains("current.pem"),
        "refused before the keys: {text}"
    );
}

#[test]
fn serve_with_an_op_block_gets_past_the_loader() {
    // The control: the same document with the method permitted loads, and stops
    // at the key files. (It names `keypair` too, so the key files are the next
    // thing it needs: `[machine_tokens]` alone, since VSA-3b, wants none.)
    let config = overlay_config(
        "op-ok",
        &format!(
            "{}auth:\n  methods: [keypair, machine_tokens]\n{OP_BLOCK}",
            config::no_providers_overlay()
        ),
    );
    let text = stderr(&serve_with(&config));
    assert!(text.contains("current.pem"), "{text}");
}

/// `provision-client` and `rotate-signing-key` ask the configuration before the
/// database, so with the method off they refuse without one: the URL below points
/// at a port nothing listens on, and a command that tried it would say so instead.
#[test]
fn the_token_commands_refuse_before_the_database_when_the_method_is_off() {
    for (case, overlay) in [
        ("default", config::no_providers_overlay()),
        (
            "keypair-only",
            format!(
                "{}auth:\n  methods: [keypair]\n",
                config::no_providers_overlay()
            ),
        ),
    ] {
        let config = overlay_config(&format!("cli-off-{case}"), &overlay);
        for (subcommand, extra) in [
            ("rotate-signing-key", Vec::<&str>::new()),
            (
                "provision-client",
                vec![
                    "--app-id",
                    "capp0000000000000000000000",
                    "--label",
                    "x",
                    "--scope",
                    "sms:send",
                    "--key-out",
                    "/nonexistent/never-written.pem",
                ],
            ),
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
                .arg(subcommand)
                .args(config.args())
                .args(["--database-url", "postgres://x:x@127.0.0.1:1/x"])
                .args(&extra)
                .env_clear()
                .output()
                .expect("running the command");
            let text = stderr(&output);
            assert!(!output.status.success(), "{case} {subcommand}");
            assert!(
                text.contains("auth_method_disabled"),
                "{case} {subcommand}: {text}"
            );
            assert!(
                text.contains("register-client"),
                "{case} {subcommand}: {text}"
            );
            assert!(
                !text.contains("connecting to Postgres"),
                "{case} {subcommand} reached the database: {text}"
            );
        }
    }
}

/// The three commands that mint a console sign-in refuse, `auth_method_disabled`,
/// **before** the database is touched, when `human_login` is not served - and they
/// write nothing. `--database-url` names a port nothing listens on, so a refusal
/// that came after connecting would be a different error (and, with the default
/// pool timeout, a slow one). Needs no Postgres.
#[test]
fn the_password_commands_refuse_before_the_database_when_human_login_is_not_served() {
    let dir = std::env::temp_dir().join(format!("sms-cutover-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(base(), dir.join("application.yml")).unwrap();
    // The keypair alone, the vaam shape: human_login is not permitted.
    std::fs::write(
        dir.join("application-test.yml"),
        "auth:\n  methods: [keypair]\n",
    )
    .unwrap();
    // The absent default permits it and writes no block: permitted is not served.
    std::fs::write(dir.join("application-default-methods.yml"), "").unwrap();
    let out = dir.join("never-written");
    let out_str = out.display().to_string();
    for profile in ["test", "default-methods"] {
        for args in [
            vec!["seed-console-client"],
            vec![
                "provision-user",
                "--email",
                "x@example.test",
                "--display-name",
                "X",
                "--role-key",
                "support",
                "--password",
                "--password-out",
                out_str.as_str(),
            ],
            vec![
                "bootstrap",
                "--skip-seed-dispatch",
                "--owner-email",
                "x@example.test",
                "--owner-display-name",
                "X",
                "--owner-password-out",
                out_str.as_str(),
            ],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
                .args(&args)
                .arg("--config")
                .arg(dir.join("application.yml"))
                .args(["--profile", profile])
                .args([
                    "--database-url",
                    "postgres://nobody:nobody@127.0.0.1:1/none",
                ])
                .env_clear()
                .env("SMS_HASH_PEPPER", PEPPER)
                .output()
                .expect("running sms-gateway");
            let text = stderr(&output);
            assert!(!output.status.success(), "{profile}: {args:?} must refuse");
            assert!(
                text.contains("auth_method_disabled"),
                "{profile}: {args:?}: {text}"
            );
            assert!(
                text.contains("pair-console-key"),
                "{profile}: {args:?}: {text}"
            );
            assert!(
                !text.contains("connecting to Postgres"),
                "{profile}: {args:?} reached the database: {text}"
            );
            assert!(!out.exists(), "{profile}: {args:?} wrote a file");
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// `provision-user` without `--password` reads no configuration at all, so the
/// command that has always worked on a bare database keeps working with no
/// `SMS_CONFIG`; the password flags are what pull the configuration in.
#[test]
fn provision_user_without_a_password_needs_no_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .args([
            "provision-user",
            "--email",
            "x@example.test",
            "--display-name",
            "X",
        ])
        .args(["--role-key", "support"])
        .args([
            "--database-url",
            "postgres://nobody:nobody@127.0.0.1:1/none",
        ])
        .env_clear()
        .output()
        .expect("running sms-gateway");
    let text = stderr(&output);
    assert!(!output.status.success());
    assert!(
        !text.contains("no config path given"),
        "the plain command must not read the configuration: {text}"
    );
    assert!(text.contains("connecting to Postgres"), "{text}");
}

/// `--password` and `--password-out` come together, and `--reset` needs them.
#[test]
fn the_password_flags_are_all_or_nothing() {
    for args in [
        vec!["--password"],
        vec!["--password-out", "/tmp/x"],
        vec!["--reset"],
        vec!["--reset", "--password-out", "/tmp/x"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
            .args([
                "provision-user",
                "--email",
                "x@example.test",
                "--display-name",
                "X",
            ])
            .args(["--role-key", "support"])
            .args([
                "--database-url",
                "postgres://nobody:nobody@127.0.0.1:1/none",
            ])
            .args(&args)
            .env_clear()
            .output()
            .expect("running sms-gateway");
        assert!(!output.status.success(), "{args:?}");
        assert!(
            stderr(&output).contains("required arguments"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}
