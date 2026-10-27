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
//!    A constant that is equal everywhere proves nothing if a crate stops using
//!    it, so each SDK's `ClientEnvelope::new(_, …)` call must also pass
//!    `AUDIENCE` itself, not a literal of its own.
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

    for file in [RUST_SDK, WASM_CORE] {
        problems.extend(envelope_audience_problems(file, &read(root, file)?));
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

/// `source` with its `//` comments removed (to the end of each line). A
/// string holding `//` loses the rest of its line, which can only make a check
/// stricter, never let a comment satisfy one.
fn without_rust_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `source` with its `#` comments removed: a `#` that starts a line or follows
/// whitespace, to the end of the line.
fn without_hash_comments(source: &str) -> String {
    let re = Regex::new(r"(^|\s)#.*$").expect("fixed pattern");
    source
        .lines()
        .map(|line| re.replace(line, "$1").into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The arguments of every `ClientEnvelope::new(…)` call in `source`, split at
/// the commas that are not inside brackets or a string.
fn envelope_calls(source: &str) -> Vec<Vec<String>> {
    const CALL: &str = "ClientEnvelope::new(";
    let mut calls = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find(CALL) {
        rest = &rest[at + CALL.len()..];
        let (mut depth, mut quoted) = (0_usize, false);
        let (mut args, mut current) = (Vec::new(), String::new());
        for c in rest.chars() {
            match c {
                '"' => {
                    quoted = !quoted;
                    current.push(c);
                }
                _ if quoted => current.push(c),
                '(' | '[' | '{' => {
                    depth += 1;
                    current.push(c);
                }
                ')' | ']' | '}' if depth > 0 => {
                    depth -= 1;
                    current.push(c);
                }
                ')' => break,
                ',' if depth == 0 => args.push(std::mem::take(&mut current)),
                _ => current.push(c),
            }
        }
        args.push(current);
        calls.push(
            args.into_iter()
                .map(|arg| arg.trim().to_owned())
                .filter(|arg| !arg.is_empty())
                .collect(),
        );
    }
    calls
}

/// Every `ClientEnvelope::new` in `file` must be given `AUDIENCE` as its
/// audience, and there must be one: a sibling literal that merely equals it
/// today is a second place to change.
fn envelope_audience_problems(file: &str, source: &str) -> Vec<String> {
    let calls = envelope_calls(&without_rust_comments(source));
    if calls.is_empty() {
        return vec![format!(
            "{file}: no `ClientEnvelope::new(_, AUDIENCE)` call, so nothing ties this SDK to the \
             audience the other crates carry"
        )];
    }
    calls
        .into_iter()
        .filter(|args| {
            args.len() != 2 || !(args[1] == "AUDIENCE" || args[1].ends_with("::AUDIENCE"))
        })
        .map(|args| {
            format!(
                "{file}: `ClientEnvelope::new({})` must pass `AUDIENCE` as its second argument",
                args.join(", ")
            )
        })
        .collect()
}

/// The quoted strings of the `key = [ … ]` array of a manifest, comments removed.
fn manifest_array(manifest: &str, key: &str) -> Vec<String> {
    let manifest = without_hash_comments(manifest);
    let array = Regex::new(&format!(r"(?ms)^\s*{key}\s*=\s*\[(.*?)\]")).expect("fixed pattern");
    let string = Regex::new(r#""([^"]*)""#).expect("fixed pattern");
    array
        .captures_iter(&manifest)
        .flat_map(|c| {
            string
                .captures_iter(&c[1])
                .map(|s| s[1].to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Whether a workspace `members`/`exclude` entry names `package` (an exact path,
/// or a glob with `*`).
fn names_package(entry: &str, package: &str) -> bool {
    let entry = entry.trim_start_matches("./");
    let pattern = format!("^{}$", regex::escape(entry).replace(r"\*", ".*"));
    Regex::new(&pattern).is_ok_and(|re| re.is_match(package))
}

fn wiring_problems(workspace_toml: &str, build_script: &str) -> Vec<String> {
    const PACKAGE: &str = "vsms-sdk-wasm";
    let mut problems = Vec::new();
    let is_member = manifest_array(workspace_toml, "members")
        .iter()
        .any(|entry| names_package(entry, PACKAGE));
    let excluded = manifest_array(workspace_toml, "exclude")
        .iter()
        .any(|entry| names_package(entry, PACKAGE));
    if !is_member || excluded {
        problems.push(format!(
            "{WORKSPACE} must list \"{PACKAGE}\" in `members` and not in `exclude`, or its \
             Cargo.lock entry and `cargo test` never cover it"
        ));
    }
    let script = without_hash_comments(build_script);
    let builds_package = Regex::new(r#"(-p|--package)[ =]+vsms-sdk-wasm(\s|"|$)"#)
        .expect("fixed pattern")
        .is_match(&script);
    if !builds_package {
        problems.push(format!(
            "{BUILD_SCRIPT} no longer builds the tested crate (`-p {PACKAGE}` is missing outside \
             its comments)"
        ));
    }
    if !script.contains("vsms_sdk_wasm") {
        problems.push(format!(
            "{BUILD_SCRIPT} no longer binds the tested crate's output (`vsms_sdk_wasm` is \
             missing outside its comments)"
        ));
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

    const SCRIPT: &str =
        "cargo build -p vsms-sdk-wasm --locked\nwasm-bindgen --out-name vsms_sdk_wasm\n";

    #[test]
    fn the_wasm_crate_must_stay_wired_to_the_workspace_and_the_build_script() {
        assert_eq!(
            wiring_problems("members = [\"vsms-sdk-wasm\"]", SCRIPT),
            Vec::<String>::new()
        );
        assert_eq!(wiring_problems("members = []", SCRIPT).len(), 1);
        assert_eq!(
            wiring_problems("members = [\"vsms-sdk-wasm\"]", "").len(),
            2
        );
    }

    #[test]
    fn a_member_that_is_also_excluded_is_not_a_member() {
        let manifest = "members = [\"vsms-sdk-rust\"]\nexclude = [\"vsms-sdk-wasm\"]";
        assert_eq!(wiring_problems(manifest, SCRIPT).len(), 1);
        let both =
            "members = [\"vsms-sdk-rust\", \"vsms-sdk-wasm\"]\nexclude = [\"vsms-sdk-wasm\"]";
        assert_eq!(wiring_problems(both, SCRIPT).len(), 1);
    }

    #[test]
    fn a_name_in_a_comment_or_another_key_is_not_a_member() {
        for manifest in [
            "members = [\"vsms-sdk-rust\"] # \"vsms-sdk-wasm\"",
            "# members = [\"vsms-sdk-wasm\"]\nmembers = [\"vsms-sdk-rust\"]",
            "default-members = [\"vsms-sdk-wasm\"]\nmembers = [\"vsms-sdk-rust\"]",
        ] {
            assert_eq!(wiring_problems(manifest, SCRIPT).len(), 1, "{manifest}");
        }
    }

    #[test]
    fn members_may_be_listed_over_several_lines_or_as_a_glob() {
        for manifest in [
            "members = [\n  \"vsms-sdk-rust\",\n  \"vsms-sdk-wasm\", # the sealer\n]",
            "members = [\"vsms-sdk-*\"]",
            "members = [\"./vsms-sdk-wasm\"]",
        ] {
            assert_eq!(
                wiring_problems(manifest, SCRIPT),
                Vec::<String>::new(),
                "{manifest}"
            );
        }
    }

    #[test]
    fn a_build_command_in_a_comment_does_not_count() {
        let members = "members = [\"vsms-sdk-wasm\"]";
        let swapped = "cargo build -p some-other-crate # was -p vsms-sdk-wasm\nwasm-bindgen --out-name vsms_sdk_wasm\n";
        assert_eq!(wiring_problems(members, swapped).len(), 1);
        let only_commented =
            "# cargo build -p vsms-sdk-wasm\n# wasm-bindgen --out-name vsms_sdk_wasm\n";
        assert_eq!(wiring_problems(members, only_commented).len(), 2);
        let prefix = "cargo build -p vsms-sdk-wasm-extra\nwasm-bindgen --out-name vsms_sdk_wasm\n";
        assert_eq!(wiring_problems(members, prefix).len(), 1);
        assert_eq!(
            wiring_problems(
                members,
                "cargo build --package vsms-sdk-wasm\nvsms_sdk_wasm\n"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_envelope_must_be_given_the_audience_constant() {
        let good = "let e = ClientEnvelope::new(cose, AUDIENCE)?;";
        assert_eq!(envelope_audience_problems("f", good), Vec::<String>::new());
        let qualified = "ClientEnvelope::new(cose, crate::core::AUDIENCE)";
        assert_eq!(
            envelope_audience_problems("f", qualified),
            Vec::<String>::new()
        );

        // The constant is left alone and a literal is passed instead.
        let literal = "ClientEnvelope::new(cose, \"vsms-staging\")";
        assert_eq!(envelope_audience_problems("f", literal).len(), 1);
        // One of two calls is wrong.
        let two = format!("{good}\nClientEnvelope::new(cose, other)");
        assert_eq!(envelope_audience_problems("f", &two).len(), 1);
        // A call whose first argument holds a comma.
        let nested = "ClientEnvelope::new(build(a, b), AUDIENCE)";
        assert_eq!(
            envelope_audience_problems("f", nested),
            Vec::<String>::new()
        );
        // No call at all, or only one in a comment.
        assert_eq!(envelope_audience_problems("f", "").len(), 1);
        let commented = "// ClientEnvelope::new(cose, AUDIENCE)\nlet x = 1;";
        assert_eq!(envelope_audience_problems("f", commented).len(), 1);
        // A trailing comma is not a third argument.
        let trailing = "ClientEnvelope::new(\n    cose,\n    AUDIENCE,\n)";
        assert_eq!(
            envelope_audience_problems("f", trailing),
            Vec::<String>::new()
        );
    }
}
