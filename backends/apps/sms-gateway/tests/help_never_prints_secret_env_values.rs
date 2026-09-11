//! `--help` never prints the *value* of a secret environment variable.
//!
//! clap renders the current value of an `env`-bound argument into help
//! output by default, so before `hide_env_values = true` landed,
//! `kubectl exec … -- sms-gateway provision-user --help` printed the whole
//! `DATABASE_URL`, password included, to whoever ran it — and to shell
//! scrollback, CI logs and pasted support output after that.
//!
//! This is the behavioural half of the guard: it runs the real binary with
//! recognisable sentinel values in the environment and asserts they do not
//! come back. `cargo xtask secret-env-args` is the static half, and covers
//! every crate in the workspace rather than only this binary.
//!
//! Subcommands are discovered from the binary's own root `--help`, not
//! listed here, so a subcommand added later is covered without anyone
//! remembering to extend a list.
use std::process::Command;

/// Deliberately recognisable, and deliberately not a real credential.
const DB_SENTINEL: &str = "SENTINELPASSWORD";
const PEPPER_SENTINEL: &str = "SENTINELPEPPER";
const ORANGE_SENTINEL: &str = "SENTINELORANGESECRET";

const SENTINELS: [&str; 3] = [DB_SENTINEL, PEPPER_SENTINEL, ORANGE_SENTINEL];

fn help(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .args(args)
        .arg("--help")
        .env(
            "DATABASE_URL",
            format!("postgres://vsms:{DB_SENTINEL}@db.example:5432/vsms"),
        )
        .env("SMS_HASH_PEPPER", PEPPER_SENTINEL)
        .env("ORANGE_CM_CLIENT_SECRET", ORANGE_SENTINEL)
        // `dotenvy` loads a developer's own `.env` in `main`, but never
        // over a variable already set — these three win regardless.
        .output()
        .expect("the binary under test is built by cargo before this test runs");

    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text
}

/// Every subcommand clap lists under `Commands:` in the root help.
fn subcommands() -> Vec<String> {
    let root = help(&[]);
    root.lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .filter_map(|l| l.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_subcommand_help_hides_secret_env_values() {
    let subcommands = subcommands();
    assert!(
        subcommands.len() > 5,
        "subcommand discovery returned {subcommands:?} — the root help format changed, \
         and this test would otherwise pass by checking nothing"
    );

    for subcommand in &subcommands {
        let text = help(&[subcommand]);
        for sentinel in SENTINELS {
            assert!(
                !text.contains(sentinel),
                "`sms-gateway {subcommand} --help` printed {sentinel} — a secret env var's \
                 value is reaching help output. Add `hide_env_values = true` to that argument."
            );
        }
    }
}

#[test]
fn hiding_the_value_still_shows_the_variable_name() {
    // The point of `hide_env_values` over dropping `env` entirely: an
    // operator still learns *which* variable feeds the flag.
    let text = help(&["provision-user"]);
    assert!(
        text.contains("[env: DATABASE_URL]"),
        "provision-user's help no longer names DATABASE_URL at all — \
         hiding the value must not cost the operator the variable name.\n{text}"
    );
}
