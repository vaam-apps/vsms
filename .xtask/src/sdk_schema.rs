//! The Rust SDK's vendored copy of `schema/schema.cstack`.
//!
//! Port of the deleted `ci/assert-sdk-schema-current.sh` (the drift check)
//! and `sdks/rust/vsms-sdk-rust/vendor-schema.sh` (the refresh). Both stay
//! together in one module because they are two views of one fact: whether
//! `sdks/rust/vsms-sdk-rust/schema.cstack` is a plain, byte-for-byte copy of
//! `schema/schema.cstack`.
//!
//! # Why a copy exists at all
//!
//! `include_client_schema!` (`cratestack-macros`) resolves its schema path
//! against the invoking crate's own `CARGO_MANIFEST_DIR`, and bakes an
//! absolute `include_str!(...)` of that resolved path into the macro
//! expansion. That path is real at this monorepo's own build time — a
//! relative `../../../schema/schema.cstack` would resolve fine — but
//! `vsms-sdk-rust` is meant to be published to crates.io and built from an
//! integrator's own cargo registry cache, where nothing above the crate's
//! own directory exists. So the schema this crate expands against has to
//! live *inside* the published package, not be reached by climbing back
//! into the monorepo.
use std::fs;
use std::path::Path;

const CANONICAL: &str = "schema/schema.cstack";
const VENDORED: &str = "sdks/rust/vsms-sdk-rust/schema.cstack";

/// `assert-sdk-schema-current.sh`: fail if the vendored copy has drifted.
pub fn check(root: &Path) -> Result<(), String> {
    let canonical_path = root.join(CANONICAL);
    let vendored_path = root.join(VENDORED);

    if !vendored_path.exists() {
        return Err(format!("sdk-schema-check: {VENDORED} is missing"));
    }

    let canonical = fs::read_to_string(&canonical_path)
        .map_err(|e| format!("{}: {e}", canonical_path.display()))?;
    let vendored = fs::read_to_string(&vendored_path)
        .map_err(|e| format!("{}: {e}", vendored_path.display()))?;

    if canonical == vendored {
        println!("sdk-schema-check: OK — the SDK's vendored schema matches {CANONICAL}");
        return Ok(());
    }

    Err(format!(
        "sdk-schema-check: the SDK's vendored schema has drifted from {CANONICAL}.\n\n\
         Refresh it with: cargo xtask sdk-schema-vendor\n\
         and commit the result in the same change as the schema edit."
    ))
}

/// `vendor-schema.sh`: refresh the vendored copy from the canonical schema.
/// A plain, verifiable copy — not a fork.
pub fn vendor(root: &Path) -> Result<(), String> {
    let canonical_path = root.join(CANONICAL);
    let vendored_path = root.join(VENDORED);

    let canonical = fs::read_to_string(&canonical_path)
        .map_err(|e| format!("{}: {e}", canonical_path.display()))?;
    fs::write(&vendored_path, &canonical)
        .map_err(|e| format!("{}: {e}", vendored_path.display()))?;

    println!(
        "vendored {} from {}",
        vendored_path.display(),
        canonical_path.display()
    );
    Ok(())
}
