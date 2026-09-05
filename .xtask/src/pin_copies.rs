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
//!
//! # Why one of the five is a `~` range rather than an `=` pin
//!
//! "Every copy is exact" was the original rule, and it was wrong for
//! exactly one of the five. `sdks/rust/vsms-sdk-rust` is the only entry
//! here that is a **published library** (`publish = true`, on crates.io);
//! everything else in this list is an application, a binary, or an image
//! build argument, and for those an exact pin binds only itself.
//!
//! Cargo resolves one version per semver-compatible line for the whole
//! graph, so `=0.11.0` in a published library is not a statement about
//! that library — it is a veto over every other crate in the consumer's
//! graph, forbidding 0.11.1 and every later patch. `vsms-sdk-rust` 0.3.1
//! shipped that veto, and a downstream workspace already on
//! `cratestack-client = "=0.11.1"` could not depend on the SDK at all
//! (`failed to select a version for cratestack-client` … `versions that
//! meet the requirements =0.11.0 are: 0.11.0`); it had to downgrade its
//! own pin to adopt the SDK.
//!
//! So the guard keeps what it was actually protecting — the *floor* of
//! every copy tracks the root pin, which is what a missed bump breaks —
//! and additionally asserts the operator each manifest is supposed to use,
//! so neither half can drift back silently: the library must say `~`, the
//! unpublished e2e binary must say `=`.
use std::fs;
use std::path::Path;

use crate::cratestack_pin;

const DOCKERFILE: &str = "ci/runner/Dockerfile";
const ARG_PREFIX: &str = "ARG CRATESTACK_VERSION=";
const CODEC_PREFIX: &str = "cratestack-codec-json = \"=";
const CLIENT_PREFIX: &str = "cratestack = { package = \"cratestack-client\", version = \"";

/// The `cratestack-client` manifests, and the requirement operator each one
/// must use — see this module's own doc for why they differ. `~` is only
/// correct for the published library; `=` is only correct for the crates
/// that ship nothing to crates.io.
const CLIENT_MANIFESTS: [(&str, &str); 2] = [
    // Published to crates.io — an exact pin here is a veto over every
    // consumer's graph, so this one carries `~X.Y.Z` (>=X.Y.Z, <X.(Y+1).0).
    ("sdks/rust/vsms-sdk-rust/Cargo.toml", "~"),
    // `publish = false`, a binary run by CI against a compose stack.
    // Nothing downstream resolves against it, so exact is free here.
    ("ci/e2e-integration/vsms-e2e-integration/Cargo.toml", "="),
];

pub fn run(root: &Path) -> Result<(), String> {
    let pinned = cratestack_pin::read_pin(root)?;
    let mut copies = vec![(
        "Cargo.toml (cratestack-codec-json)".to_owned(),
        quoted_version(&read(root, "Cargo.toml")?, CODEC_PREFIX),
    )];
    for (manifest, op) in CLIENT_MANIFESTS {
        copies.push((
            format!("{manifest} (cratestack-client, must be `{op}`)"),
            client_version(&read(root, manifest)?, op),
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
         Bump every one to {pinned}, keeping each client manifest's own required \
         operator (`~` for the published SDK, `=` for the e2e binary — see \
         .xtask/src/pin_copies.rs); a mismatched client manifest is invisible to \
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

/// The version out of a `cratestack-client` manifest line, but only when the
/// requirement uses the operator that manifest is required to use.
///
/// Returning `None` on the wrong operator is deliberate: it lands in the same
/// "no pin line found" branch a missing line does, which is the accurate
/// report — the line the guard is looking for (`version = "<op>X.Y.Z"`) is
/// genuinely not there.
fn client_version(text: &str, op: &str) -> Option<String> {
    quoted_version(text, CLIENT_PREFIX)?
        .strip_prefix(op)
        .map(str::to_owned)
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
            Some("=0.11.0")
        );
    }

    #[test]
    fn reads_the_floor_of_each_allowed_operator() {
        let tilde = "cratestack = { package = \"cratestack-client\", version = \"~0.11.0\", default-features = false }\n";
        assert_eq!(client_version(tilde, "~").as_deref(), Some("0.11.0"));
        let exact = "cratestack = { package = \"cratestack-client\", version = \"=0.11.0\", default-features = false }\n";
        assert_eq!(client_version(exact, "=").as_deref(), Some("0.11.0"));
    }

    /// The published SDK regressing to `=` is the defect this operator rule
    /// exists to catch, so prove the guard actually sees it — a `~`-required
    /// manifest that says `=` reads as no pin line at all, and `run` reports it.
    #[test]
    fn rejects_the_wrong_operator_in_either_direction() {
        let exact = "cratestack = { package = \"cratestack-client\", version = \"=0.11.0\", default-features = false }\n";
        assert_eq!(client_version(exact, "~"), None);
        let tilde = "cratestack = { package = \"cratestack-client\", version = \"~0.11.0\", default-features = false }\n";
        assert_eq!(client_version(tilde, "="), None);
        // A caret is not one of the two either. On today's 0.11 line `^`
        // and `~` resolve identically (Cargo treats a leading zero as the
        // significant component), so this is not about today's behaviour:
        // it is about the day cratestack reaches 1.0, when `^` silently
        // becomes "any 1.x" while `~` stays "this minor line". The rule
        // names one spelling so that divergence is a review, not a surprise.
        let caret = "cratestack = { package = \"cratestack-client\", version = \"^0.11.0\", default-features = false }\n";
        assert_eq!(client_version(caret, "~"), None);
    }
}
