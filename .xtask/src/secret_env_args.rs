//! Every clap argument bound to a credential-bearing environment variable
//! carries `hide_env_values = true`.
//!
//! clap renders the *current value* of an `env`-bound argument into
//! `--help` by default, so `sms-gateway provision-user --help` inside a
//! pod printed the whole `DATABASE_URL`, password included — see the
//! issue this check landed with. The attribute keeps the variable NAME
//! (the useful half) and drops its value.
//!
//! This is deliberately a static check rather than only a runtime test:
//! the defect is introduced by *adding an argument*, in any crate, and a
//! runtime test can only cover binaries someone remembered to write one
//! for. [`SECRET_ENV_VARS`] is the enforced list — a variable belongs on
//! it when its value is credential material, not when it is merely
//! configuration. A client id, an issuer URL, a sender number, a bind
//! address or a backup destination stays visible on purpose: an operator
//! reading `--help` is usually doing so because they do not yet know how
//! to invoke the command, and hiding the harmless values makes that
//! worse for no gain.
use std::fs;
use std::path::{Path, PathBuf};

/// Variables whose value is credential material. Adding one here makes
/// every existing `#[arg(..., env = "…")]` binding it a hard failure
/// until it also carries `hide_env_values = true`.
const SECRET_ENV_VARS: [&str; 3] = ["DATABASE_URL", "SMS_HASH_PEPPER", "ORANGE_CM_CLIENT_SECRET"];

/// Directory names never worth walking into — build output, VCS
/// internals, agent worktrees (each a full second copy of this repo,
/// whose stale sources would otherwise be reported as live violations)
/// and installed JS packages.
const SKIP_DIRS: [&str; 4] = ["target", ".git", ".claude", "node_modules"];

pub fn run(root: &Path) -> Result<(), String> {
    let mut hits = Vec::new();
    let mut scanned = 0usize;

    for file in rust_sources(root) {
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        scanned += 1;
        for (line, attr) in arg_attributes(&text) {
            let Some(var) = SECRET_ENV_VARS
                .iter()
                .find(|v| attr.contains(&format!("env = \"{v}\"")))
            else {
                continue;
            };
            if !attr.contains("hide_env_values = true") {
                hits.push(format!(
                    "{rel}:{line}: env = \"{var}\" without hide_env_values"
                ));
            }
        }
    }

    if hits.is_empty() {
        println!(
            "secret-env-args OK ({scanned} files; guarded: {})",
            SECRET_ENV_VARS.join(" ")
        );
        return Ok(());
    }

    eprintln!("secret-env-args violation — clap would print these values in --help:");
    for hit in &hits {
        eprintln!("{hit}");
    }
    eprintln!();
    eprintln!("Add `hide_env_values = true` to the #[arg(...)] attribute. clap keeps");
    eprintln!("showing the variable name and stops printing its current value.");
    Err("secret-env-args violation".to_owned())
}

/// Every `#[arg(...)]` attribute in `text`, as `(1-based line, body)`.
///
/// Scans for the literal `#[arg(` and consumes to its balanced closing
/// paren, so a multi-line attribute is returned as one string — matching
/// per line would miss `env = "…"` and `hide_env_values = true` sitting
/// on different lines of the same attribute. String literals are skipped
/// while balancing, so a paren inside a `default_value` cannot end the
/// attribute early.
fn arg_attributes(text: &str) -> Vec<(usize, String)> {
    const OPEN: &str = "#[arg(";
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut search = 0usize;

    while let Some(found) = text[search..].find(OPEN) {
        let start = search + found;
        let body_start = start + OPEN.len();
        let mut depth = 1i32;
        let mut i = body_start;
        let mut in_string = false;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' if in_string => i += 1,
                b'"' => in_string = !in_string,
                b'(' if !in_string => depth += 1,
                b')' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let end = i.min(bytes.len());
        let line = text[..start].matches('\n').count() + 1;
        out.push((line, text[body_start..end].to_owned()));
        search = end.max(body_start);
    }

    out
}

/// Every `*.rs` file under `dir`, recursively, skipping [`SKIP_DIRS`].
fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            let skip = path
                .file_name()
                .is_some_and(|n| SKIP_DIRS.iter().any(|s| n == *s));
            if !skip {
                out.extend(rust_sources(&path));
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_line_attribute_is_read_as_one_unit() {
        let src =
            "#[arg(\n    long,\n    env = \"DATABASE_URL\",\n    hide_env_values = true\n)]\n";
        let attrs = arg_attributes(src);
        assert_eq!(attrs.len(), 1);
        assert!(attrs[0].1.contains("env = \"DATABASE_URL\""));
        assert!(attrs[0].1.contains("hide_env_values = true"));
    }

    #[test]
    fn a_paren_inside_a_string_literal_does_not_end_the_attribute() {
        let src = "#[arg(long, default_value = \")\", env = \"DATABASE_URL\")]\n";
        let attrs = arg_attributes(src);
        assert_eq!(attrs.len(), 1);
        assert!(attrs[0].1.contains("env = \"DATABASE_URL\""));
    }

    #[test]
    fn a_nested_call_in_default_value_t_does_not_end_the_attribute() {
        let src = "#[arg(long, env = \"SMS_HASH_PEPPER\", default_value_t = f(g()))]\n";
        let attrs = arg_attributes(src);
        assert_eq!(attrs.len(), 1);
        assert!(attrs[0].1.contains("env = \"SMS_HASH_PEPPER\""));
        assert!(!attrs[0].1.contains("hide_env_values = true"));
    }

    #[test]
    fn the_line_number_points_at_the_attribute() {
        let src = "struct A {\n    /// doc\n    #[arg(long, env = \"DATABASE_URL\")]\n    a: String,\n}\n";
        let attrs = arg_attributes(src);
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].0, 3);
    }

    #[test]
    fn this_repository_has_no_violations() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect(".xtask always has a parent");
        run(root).expect("no secret-bearing env arg may print its value in --help");
    }
}
