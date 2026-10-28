//! No committed livemode profile carries a literal where a secret belongs.
//!
//! `sms-config` refuses a literal secret under `deployment.livemode: true` when
//! it loads a document, but only at boot, against one deployment's merged files.
//! This is the static half: it reads every `config/application*.yml` as text, so
//! a literal committed to a livemode overlay fails the pull request that adds it
//! rather than the first production boot that reads it. It depends on no YAML
//! parser, on purpose: this crate has none (only `regex`) and stays fast to
//! build, so it reads text, one line at a time.
//!
//! What a line-based scan sees, and what it does not:
//!
//! - a key written in a block (`client_secret: x`) or in a flow mapping
//!   (`credentials: {client_id: a, client_secret: x}`), quoted or not, and a
//!   flow mapping that spans lines, since each key is found on its own line;
//! - `livemode: true` in any spelling YAML reads as true (`True`, `TRUE`), in a
//!   block or a flow mapping;
//! - NOT a value that is only a literal once YAML has folded it: a `|` or `>`
//!   block scalar, a multi-line plain scalar, an anchor that reaches the key
//!   from elsewhere (an `*alias` value is reported, conservatively, as a
//!   literal), or a secret under a key that is spelt through an escape.
//!
//! None of that is a hole in the product: `sms-config` parses the real document
//! at boot and refuses every one of them. This guard is the cheap early half and
//! the loader is the authority, so a document this scan passes can still be
//! refused at boot, and never the other way round.
//!
//! [`SECRET_KEYS`] is the same list `sms-config` checks (`security.hash_pepper`,
//! and each provider's `credentials.client_secret` / `secret_access_key`). A
//! client id or an access-key id is an identifier, not a secret, and may be a
//! literal.
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

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

/// A secret key and the rest of its value on this line: up to the next `,` or
/// `}` (a flow mapping's separators) or the end of the line, a `${NAME}` counting
/// as part of the value and not as the end of a mapping. The key may be quoted
/// and may follow a `{` or a `,`.
static SECRET_ENTRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?:^|[\s{{,])["']?({})["']?\s*:\s*((?:\$\{{[^}}]*\}}|[^,}}])*)"#,
        SECRET_KEYS.join("|")
    ))
    .expect("the secret-entry pattern is valid")
});

/// `livemode` set to a YAML true: `true`, `True` or `TRUE`, as a block entry or
/// inside a flow mapping.
static LIVEMODE_TRUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|[\s{,])["']?livemode["']?\s*:\s*(?i:true)\s*(?:$|[,}])"#)
        .expect("the livemode pattern is valid")
});

/// Whether the file sets `livemode: true` outside a comment.
fn declares_livemode(text: &str) -> bool {
    text.lines()
        .any(|line| LIVEMODE_TRUE.is_match(strip_comment(line)))
}

/// `(1-based line, key)` of every secret key whose value is not exactly one
/// `${NAME}` placeholder.
fn literal_secrets(text: &str) -> Vec<(usize, &'static str)> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = strip_comment(line);
        for entry in SECRET_ENTRY.captures_iter(line) {
            let Some(secret) = SECRET_KEYS
                .iter()
                .find(|candidate| **candidate == &entry[1])
            else {
                continue;
            };
            if !is_placeholder(entry[2].trim()) {
                found.push((index + 1, *secret));
            }
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
    fn a_literal_in_a_flow_mapping_is_reported() {
        assert_eq!(
            literal_secrets("    credentials: {client_id: a, client_secret: hunter2}\n"),
            vec![(1, "client_secret")]
        );
        assert_eq!(
            literal_secrets("    credentials: {client_secret: hunter2, client_id: a}\n"),
            vec![(1, "client_secret")]
        );
        assert_eq!(
            literal_secrets("    credentials: { \"secret_access_key\": 'k' }\n"),
            vec![(1, "secret_access_key")]
        );
    }

    #[test]
    fn a_placeholder_in_a_flow_mapping_is_not_a_literal() {
        assert_eq!(
            literal_secrets("    credentials: {client_id: a, client_secret: ${X}}\n"),
            none()
        );
        assert_eq!(
            literal_secrets("    credentials: {client_secret: \"${X}\", client_id: a}\n"),
            none()
        );
    }

    #[test]
    fn a_flow_mapping_across_lines_is_read_key_by_key() {
        let text =
            "    credentials: {\n      client_id: a,\n      client_secret: hunter2,\n    }\n";
        assert_eq!(literal_secrets(text), vec![(3, "client_secret")]);
    }

    #[test]
    fn a_key_that_merely_contains_a_secret_key_is_not_one() {
        assert_eq!(
            literal_secrets("      not_client_secret: x\n      client_secret_id: y\n"),
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
    fn livemode_is_true_in_every_spelling_yaml_reads_as_true() {
        for text in [
            "  livemode: true\n",
            "  livemode: True\n",
            "  livemode: TRUE  # live\n",
            "deployment: {name: x, livemode: True}\n",
            "deployment: {livemode: true}\n",
            "deployment: {\"livemode\": true, name: x}\n",
        ] {
            assert!(declares_livemode(text), "{text}");
        }
        for text in [
            "  livemode: false\n",
            "  livemode: truest\n",
            "  not_livemode: true\n",
            "deployment: {livemode: False}\n",
        ] {
            assert!(!declares_livemode(text), "{text}");
        }
    }

    #[test]
    fn this_repository_has_no_violations() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect(".xtask always has a parent");
        run(root).expect("no committed livemode profile may carry a literal secret");
    }
}
