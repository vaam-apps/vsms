//! The single place that reads the pinned `cratestack` version out of the
//! root `Cargo.toml`.
//!
//! Port of the deleted `ci/cratestack-pin.sh`. Prints the version (e.g.
//! `0.7.10`) to stdout and nothing else on success — CI captures that with
//! `$GITHUB_OUTPUT`, so stdout must stay exactly one bare version string,
//! matching the original script's own contract.
//!
//! #204's own review of the original bash version is the reason this is a
//! single shared place at all rather than three copies of the same `sed`
//! expression: a duplicated *extraction* of one value is the milder form of
//! the duplicated-hardcoded-list smell `AGENTS.md`'s release-engineering
//! notes warn about, and it had already drifted once (three regexes to
//! keep in lockstep instead of one). `migrations_current` calls this
//! directly in-process now, rather than shelling back out to a second
//! `cargo xtask cratestack-pin` invocation — one fewer process, same single
//! source of truth.
use std::fs;
use std::path::Path;

/// Reads `Cargo.toml` and returns the version pinned for
/// `cratestack = { package = "cratestack-pg", version = "=X.Y.Z" }` — the
/// exact line shape the original `sed` expression matched.
pub fn read_pin(root: &Path) -> Result<String, String> {
    let path = root.join("Cargo.toml");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_pin(&text).ok_or_else(|| {
        format!(
            "could not read the cratestack version pin from {}\n\
             expected a line shaped like: cratestack = {{ package = \"cratestack-pg\", version = \"=X.Y.Z\" }}",
            path.display()
        )
    })
}

fn parse_pin(cargo_toml: &str) -> Option<String> {
    let line = cargo_toml
        .lines()
        .find(|l| l.starts_with("cratestack = { package = \"cratestack-pg\", version = \""))?;
    let after = line.strip_prefix("cratestack = { package = \"cratestack-pg\", version = \"=")?;
    let end = after.find('"')?;
    Some(after[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_pinned_version() {
        let toml = "cratestack = { package = \"cratestack-pg\", version = \"=0.7.10\" }\n";
        assert_eq!(parse_pin(toml).as_deref(), Some("0.7.10"));
    }

    #[test]
    fn returns_none_when_the_line_is_absent() {
        assert_eq!(parse_pin("[workspace]\n"), None);
    }
}
