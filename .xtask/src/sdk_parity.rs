//! The two SDKs must bind the same bytes: the protocol constants and the
//! operation table cannot be held twice.
//!
//! # Why this exists
//!
//! The Rust SDK (`sdks/rust/vsms-sdk-rust`) seals through cratestack's
//! `ClientEnvelope`, and so does the Node SDK, which calls the same envelope
//! compiled to WebAssembly (`sdks/rust/vsms-sdk-wasm`, VS-7). A signature binds
//! the audience, the route, the payload type and the called operation's
//! contract digest, byte for byte; two SDKs that disagree about one of those do
//! not fail to compile, they fail at the gateway as the coarse unsigned `401`,
//! which says nothing about which check it was.
//!
//! The *operation table* is shared by construction: the wasm crate expands
//! `include_client_schema!` over the Rust SDK's own vendored schema, so there is
//! no second table to drift (and [`crate::sdk_schema`] holds that one file to
//! `schemas/vsms.cstack`). This check guards the two things that construction
//! does not cover:
//!
//! 1. **The audience** is a string literal in three crates (the server's
//!    `sms_api::envelope::AUDIENCE`, the Rust SDK's `AUDIENCE` and the wasm
//!    crate's), because the SDK crates must stay free of the server's. A
//!    request sealed for another audience is refused, so the three are held
//!    equal here.
//! 2. **The wasm crate keeps using the vendored schema and nothing else.** A
//!    `.cstack` file of its own, or an `include_client_schema!` over any other
//!    path, would be the second table the construction exists to prevent; and
//!    the wasm crate must stay a member of the `sdks/rust` workspace and be the
//!    package the Node SDK's build script builds, or the Node SDK would be
//!    building something else than what is tested.

use std::fs;
use std::path::Path;

use regex::Regex;

const SERVER: &str = "backends/crates/sms-api/src/envelope.rs";
const RUST_SDK: &str = "sdks/rust/vsms-sdk-rust/src/client.rs";
const WASM_CORE: &str = "sdks/rust/vsms-sdk-wasm/src/core.rs";
const WASM_CONTRACTS: &str = "sdks/rust/vsms-sdk-wasm/src/contracts.rs";
const WASM_DIR: &str = "sdks/rust/vsms-sdk-wasm";
const WORKSPACE: &str = "sdks/rust/Cargo.toml";
const BUILD_SCRIPT: &str = "sdks/node/vsms-sdk-node/scripts/build-wasm.sh";
const VENDORED_FROM_WASM: &str = "../vsms-sdk-rust/schema.cstack";

pub fn run(root: &Path) -> Result<(), String> {
    let mut problems = Vec::new();

    let audiences = [SERVER, RUST_SDK, WASM_CORE]
        .into_iter()
        .map(|file| Ok((file, audience(&read(root, file)?, file)?)))
        .collect::<Result<Vec<_>, String>>()?;
    if audiences.iter().any(|(_, value)| value != &audiences[0].1) {
        problems.push(format!(
            "the audience differs between the server and the SDKs:\n{}",
            audiences
                .iter()
                .map(|(file, value)| format!("    {file}: {value:?}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    problems.extend(wasm_schema_problems(
        &read(root, WASM_CONTRACTS)?,
        &cstack_files(&root.join(WASM_DIR)),
    ));
    problems.extend(wiring_problems(
        &read(root, WORKSPACE)?,
        &read(root, BUILD_SCRIPT)?,
    ));

    if problems.is_empty() {
        println!(
            "sdk-parity-check: OK — audience {:?} in the server and both SDKs, and the wasm crate \
             expands the Rust SDK's vendored schema alone",
            audiences[0].1
        );
        return Ok(());
    }
    Err(format!(
        "sdk-parity-check: the SDKs no longer bind the same bytes:\n\n  {}\n\n\
         See .xtask/src/sdk_parity.rs for what each rule protects.",
        problems.join("\n  ")
    ))
}

fn read(root: &Path, rel: &str) -> Result<String, String> {
    let path = root.join(rel);
    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The value of `const AUDIENCE: &str = "…";` in `source`.
fn audience(source: &str, file: &str) -> Result<String, String> {
    let re = Regex::new(r#"(?m)^\s*(?:pub(?:\([a-z]+\))? )?const AUDIENCE: &str = "([^"]*)";"#)
        .expect("fixed pattern");
    re.captures(source)
        .map(|c| c[1].to_owned())
        .ok_or_else(|| format!("{file}: no `const AUDIENCE: &str = \"…\";` found"))
}

/// Every `*.cstack` under `dir`, so a vendored second copy is seen wherever it
/// is put.
fn cstack_files(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "target") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "cstack") {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

fn wasm_schema_problems(contracts_rs: &str, own_schemas: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let re = Regex::new(r#"include_client_schema!\(\s*"([^"]*)""#).expect("fixed pattern");
    let paths: Vec<&str> = re
        .captures_iter(contracts_rs)
        .map(|c| c.get(1).map_or("", |m| m.as_str()))
        .collect();
    if paths != [VENDORED_FROM_WASM] {
        problems.push(format!(
            "{WASM_CONTRACTS} must expand `include_client_schema!` over {VENDORED_FROM_WASM:?} \
             alone, found {paths:?}"
        ));
    }
    for schema in own_schemas {
        problems.push(format!(
            "{schema}: the wasm crate holds a schema of its own; it must read the Rust SDK's \
             vendored copy so there is one operation table"
        ));
    }
    problems
}

fn wiring_problems(workspace_toml: &str, build_script: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if !workspace_toml.contains("\"vsms-sdk-wasm\"") {
        problems.push(format!(
            "{WORKSPACE} does not list \"vsms-sdk-wasm\" as a member, so its Cargo.lock entry and \
             `cargo test` never cover it"
        ));
    }
    for needle in ["-p vsms-sdk-wasm", "vsms_sdk_wasm"] {
        if !build_script.contains(needle) {
            problems.push(format!(
                "{BUILD_SCRIPT} no longer builds the tested crate (`{needle}` is missing)"
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_audience_is_read_whatever_its_visibility() {
        for source in [
            "pub const AUDIENCE: &str = \"vsms\";",
            "const AUDIENCE: &str = \"vsms\";",
            "    pub(crate) const AUDIENCE: &str = \"vsms\";",
        ] {
            assert_eq!(audience(source, "f").as_deref(), Ok("vsms"), "{source}");
        }
        assert!(audience("const OTHER: &str = \"vsms\";", "f").is_err());
    }

    #[test]
    fn only_the_vendored_schema_is_accepted_for_the_wasm_crate() {
        let good = "cratestack::include_client_schema!(\"../vsms-sdk-rust/schema.cstack\", decimal = RustDecimal);";
        assert_eq!(wasm_schema_problems(good, &[]), Vec::<String>::new());

        let other = good.replace("../vsms-sdk-rust/schema.cstack", "schema.cstack");
        assert_eq!(wasm_schema_problems(&other, &[]).len(), 1);

        let twice = format!("{good}\n{good}");
        assert_eq!(wasm_schema_problems(&twice, &[]).len(), 1);

        assert_eq!(
            wasm_schema_problems(good, &["a/schema.cstack".to_owned()]).len(),
            1
        );
    }

    #[test]
    fn the_wasm_crate_must_stay_wired_to_the_workspace_and_the_build_script() {
        let script = "cargo build -p vsms-sdk-wasm; wasm-bindgen --out-name vsms_sdk_wasm";
        assert_eq!(
            wiring_problems("members = [\"vsms-sdk-wasm\"]", script),
            Vec::<String>::new()
        );
        assert_eq!(wiring_problems("members = []", script).len(), 1);
        assert_eq!(
            wiring_problems("members = [\"vsms-sdk-wasm\"]", "").len(),
            2
        );
    }
}
