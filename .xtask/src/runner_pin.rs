//! `ci/runner/Dockerfile`'s `ARG CRATESTACK_VERSION=…` default must equal the
//! pin `cratestack_pin::read_pin` reads from `Cargo.toml`.
//!
//! The Dockerfile cannot read the pin itself (a build `ARG` default is a
//! literal), so it is a second copy of one number. The cratestack 0.11.0
//! bump raised the pin and the runner's Rust toolchain and missed this
//! line, so `just ci` built a runner carrying the 0.8.10 CLI and its own
//! step 1 refused to run — the right outcome, one image build too late.
//! This check catches the same drift from `cargo xtask`, before a build.
use std::fs;
use std::path::Path;

use crate::cratestack_pin;

const DOCKERFILE: &str = "ci/runner/Dockerfile";
const ARG_PREFIX: &str = "ARG CRATESTACK_VERSION=";

pub fn run(root: &Path) -> Result<(), String> {
    let pinned = cratestack_pin::read_pin(root)?;
    let path = root.join(DOCKERFILE);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let default = parse_default(&text)
        .ok_or_else(|| format!("{DOCKERFILE}: no line shaped like `{ARG_PREFIX}X.Y.Z`"))?;
    if default == pinned {
        println!("OK — {DOCKERFILE} defaults CRATESTACK_VERSION to the pin ({pinned})");
        return Ok(());
    }
    Err(format!(
        "{DOCKERFILE} defaults CRATESTACK_VERSION to {default}, but Cargo.toml pins {pinned}.\n\
         Update the `{ARG_PREFIX}` line to {pinned} so `just ci` builds a runner whose \
         cratestack CLI matches the library it compiles against."
    ))
}

fn parse_default(dockerfile: &str) -> Option<String> {
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
        assert_eq!(parse_default(text).as_deref(), Some("0.11.0"));
    }

    #[test]
    fn returns_none_without_the_arg() {
        assert_eq!(parse_default("FROM rust\n"), None);
    }
}
