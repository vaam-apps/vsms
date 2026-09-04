//! Every second copy of the cratestack pin must equal the one
//! `cratestack_pin::read_pin` reads from `Cargo.toml`.
//!
//! The pin is written in five places that no single `cargo` invocation
//! sees together: the `cratestack-pg` line `read_pin` parses, the
//! `cratestack-codec-json` line beside it, the two `cratestack-client`
//! manifests excluded from the workspace (`sdks/rust/vsms-sdk-rust`,
//! `ci/e2e-integration/vsms-e2e-integration`), and `ci/runner/Dockerfile`'s
//! `ARG CRATESTACK_VERSION=` default, which a build `ARG` cannot derive.
//! The 0.11.0 bump raised the first and missed the last, so `just ci` built
//! a runner whose own step 1 refused to run — the right outcome, one image
//! build too late — and the excluded manifests are exactly where a missed
//! bump stays green in CI. This check reads all five and fails on any
//! disagreement, naming each.
use std::fs;
use std::path::Path;

use crate::cratestack_pin;

const DOCKERFILE: &str = "ci/runner/Dockerfile";
const ARG_PREFIX: &str = "ARG CRATESTACK_VERSION=";
const CODEC_PREFIX: &str = "cratestack-codec-json = \"=";
const CLIENT_PREFIX: &str = "cratestack = { package = \"cratestack-client\", version = \"=";
const CLIENT_MANIFESTS: [&str; 2] = [
    "sdks/rust/vsms-sdk-rust/Cargo.toml",
    "ci/e2e-integration/vsms-e2e-integration/Cargo.toml",
];

pub fn run(root: &Path) -> Result<(), String> {
    let pinned = cratestack_pin::read_pin(root)?;
    let mut copies = vec![(
        "Cargo.toml (cratestack-codec-json)".to_owned(),
        quoted_version(&read(root, "Cargo.toml")?, CODEC_PREFIX),
    )];
    for manifest in CLIENT_MANIFESTS {
        copies.push((
            format!("{manifest} (cratestack-client)"),
            quoted_version(&read(root, manifest)?, CLIENT_PREFIX),
        ));
    }
    copies.push((
        format!("{DOCKERFILE} ({ARG_PREFIX}…)"),
        arg_default(&read(root, DOCKERFILE)?),
    ));

    let mut problems = Vec::new();
    for (place, found) in &copies {
        match found {
            Some(v) if v == &pinned => {}
            Some(v) => problems.push(format!("  {place}: {v}")),
            None => problems.push(format!("  {place}: no pin line found")),
        }
    }
    if problems.is_empty() {
        println!(
            "OK — {} copies of the cratestack pin all say {pinned}",
            copies.len() + 1
        );
        return Ok(());
    }
    Err(format!(
        "Cargo.toml pins cratestack {pinned}, but these copies disagree:\n{}\n\
         Bump every one to {pinned}; a mismatched client manifest is invisible to \
         `cargo check --workspace`, and a mismatched Dockerfile ARG builds a `just ci` \
         runner whose cratestack CLI its own first step then refuses.",
        problems.join("\n")
    ))
}

fn read(root: &Path, rel: &str) -> Result<String, String> {
    let path = root.join(rel);
    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The version inside `<prefix>X.Y.Z"` on the first line starting with `prefix`.
fn quoted_version(text: &str, prefix: &str) -> Option<String> {
    let after = text.lines().find_map(|l| l.strip_prefix(prefix))?;
    let end = after.find('"')?;
    Some(after[..end].to_owned())
}

fn arg_default(dockerfile: &str) -> Option<String> {
    dockerfile
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix(ARG_PREFIX))
        .map(|v| v.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_arg_default() {
        let text = "FROM rust\nARG CRATESTACK_VERSION=0.11.0\nRUN true\n";
        assert_eq!(arg_default(text).as_deref(), Some("0.11.0"));
    }

    #[test]
    fn returns_none_without_the_arg() {
        assert_eq!(arg_default("FROM rust\n"), None);
    }

    #[test]
    fn parses_a_quoted_manifest_version() {
        let toml = "cratestack-codec-json = \"=0.11.0\"\n";
        assert_eq!(
            quoted_version(toml, CODEC_PREFIX).as_deref(),
            Some("0.11.0")
        );
        let client = "cratestack = { package = \"cratestack-client\", version = \"=0.11.0\", default-features = false }\n";
        assert_eq!(
            quoted_version(client, CLIENT_PREFIX).as_deref(),
            Some("0.11.0")
        );
    }
}
