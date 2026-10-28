//! No committed livemode profile carries a literal where a secret belongs.
//!
//! `sms-config` refuses a literal secret under `deployment.livemode: true` when
//! it loads a document, but only at boot, against one deployment's merged files.
//! This is the static half: it reads every `config/application*.yml` as text, so
//! a literal committed to a livemode overlay fails the pull request that adds it
//! rather than the first production boot that reads it. It depends on no YAML
//! parser, on purpose: this crate stays fast to build, and the question is a
//! line-shaped one.
//!
//! [`SECRET_KEYS`] is the same list `sms-config` checks (`security.hash_pepper`,
//! and each provider's `credentials.client_secret` / `secret_access_key`). A
//! client id or an access-key id is an identifier, not a secret, and may be a
//! literal.
use std::fs;
use std::path::Path;

/// The keys whose value is secret material.
const SECRET_KEYS: [&str; 3] = ["hash_pepper", "client_secret", "secret_access_key"];

pub fn run(root: &Path) -> Result<(), String> {
    let dir = root.join("config");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .map_err(|error| format!("reading {}: {error}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("application")
                        && Path::new(name).extension().is_some_and(|ext| ext == "yml")
                })
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "no config/application*.yml under {}: the guard would pass over nothing",
            dir.display()
        ));
    }

    let mut hits = Vec::new();
    let mut livemode_files = 0usize;
    for file in &files {
        let text = fs::read_to_string(file)
            .map_err(|error| format!("reading {}: {error}", file.display()))?;
        let rel = format!(
            "config/{}",
            file.file_name().and_then(|n| n.to_str()).unwrap_or("?")
        );
        let found = literal_secrets(&text);
        if declares_livemode(&text) {
            livemode_files += 1;
            hits.extend(found.into_iter().map(|(line, key)| {
                format!("{rel}:{line}: `{key}` is a literal in a livemode profile")
            }));
        }
    }

    if hits.is_empty() {
        println!(
            "config-literal-secrets OK ({} files, {livemode_files} livemode; guarded keys: {})",
            files.len(),
            SECRET_KEYS.join(" ")
        );
        return Ok(());
    }
    eprintln!("config-literal-secrets violation - a livemode profile commits a secret:");
    for hit in &hits {
        eprintln!("{hit}");
    }
    eprintln!();
    eprintln!("Write the value as a ${{VAR}} placeholder and deliver it through the environment.");
    Err("config-literal-secrets violation".to_owned())
}

/// Whether the file sets `livemode: true` outside a comment.
fn declares_livemode(text: &str) -> bool {
    text.lines().any(|line| {
        let line = strip_comment(line);
        line.trim() == "livemode: true"
    })
}

/// `(1-based line, key)` of every secret key whose value is not exactly one
/// `${NAME}` placeholder.
fn literal_secrets(text: &str) -> Vec<(usize, &'static str)> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = strip_comment(line);
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        let Some(secret) = SECRET_KEYS
            .iter()
            .find(|candidate| **candidate == key.trim())
        else {
            continue;
        };
        if !is_placeholder(value.trim()) {
            found.push((index + 1, *secret));
        }
    }
    found
}

/// `line` without a trailing ` # comment` (or a whole-line one).
fn strip_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        return "";
    }
    line.split_once(" #").map_or(line, |(code, _)| code)
}

fn is_placeholder(value: &str) -> bool {
    let value = value.trim_matches(|c| c == '"' || c == '\'');
    value
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No literal found.
    fn none() -> Vec<(usize, &'static str)> {
        Vec::new()
    }

    #[test]
    fn a_placeholder_is_not_a_literal() {
        assert_eq!(
            literal_secrets("  client_secret: ${ORANGE_CM_CLIENT_SECRET}\n"),
            none()
        );
        assert_eq!(
            literal_secrets("  hash_pepper: \"${SMS_HASH_PEPPER}\"  # the pepper\n"),
            none()
        );
    }

    #[test]
    fn a_literal_is_reported_at_its_line() {
        let text = "security:\n  hash_pepper: abcdef\nproviders:\n  - code: x\n    credentials:\n      client_secret: hunter2\n";
        assert_eq!(
            literal_secrets(text),
            vec![(2, "hash_pepper"), (6, "client_secret")]
        );
    }

    #[test]
    fn a_placeholder_embedded_in_text_is_still_a_literal() {
        // `prefix-${X}` is a literal prefix glued onto a secret.
        assert_eq!(
            literal_secrets("      secret_access_key: key-${AWS_KEY}\n"),
            vec![(1, "secret_access_key")]
        );
    }

    #[test]
    fn an_identifier_may_be_a_literal() {
        assert_eq!(
            literal_secrets("      client_id: orange-id\n      access_key_id: AKIDEXAMPLE\n"),
            none()
        );
    }

    #[test]
    fn a_commented_literal_is_not_read() {
        assert_eq!(literal_secrets("  # client_secret: hunter2\n"), none());
    }

    #[test]
    fn livemode_is_read_outside_comments_only() {
        assert!(declares_livemode("deployment:\n  livemode: true\n"));
        assert!(!declares_livemode("deployment:\n  livemode: false\n"));
        assert!(!declares_livemode("# livemode: true\n"));
    }

    #[test]
    fn this_repository_has_no_violations() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect(".xtask always has a parent");
        run(root).expect("no committed livemode profile may carry a literal secret");
    }
}
