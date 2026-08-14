//! Every filesystem path a GitHub Actions workflow names must exist.
//!
//! # Why this exists
//!
//! The `release` workflow broke on `bd40c8b` (the vpay-layout restructure,
//! #271) and stayed broken across four subsequent merges to `main`. Five of
//! its six `dockerfile:` entries still pointed at the pre-restructure
//! `app/sms-gateway/Dockerfile` / `admin/Dockerfile` paths, so every image
//! build failed with `lstat app: no such file or directory`.
//!
//! The restructure PR updated `ci.yml` and missed `release.yml`, and
//! **nothing could have caught it**: `release` triggers only on push to
//! `main` and on `v*.*.*` tags, so no pull request ever runs it. The first
//! execution of the changed code is necessarily after it has merged. That
//! is the same shape as the pre-#118 problem of live suites CI never ran,
//! and as `AGENTS.md`'s own `#87` — a check that cannot run before merge is
//! not a check.
//!
//! A path reference is the part of a workflow that *can* be verified
//! statically, cheaply, from any branch. So it is.
//!
//! # What is checked
//!
//! Two keys, both of which name something that must already exist in the
//! repository at the time the workflow runs:
//!
//! - `dockerfile:` — a matrix entry naming a build file.
//! - `working-directory:` — a step's `cd` target.
//!
//! Deliberately **not** checked: `file:`, `context:`, `path:`, `run:` and
//! everything else. `file: ${{ matrix.dockerfile }}` is an expression, not a
//! path; `context: .` is trivially true; `path:` under `upload-artifact` may
//! legitimately name something a previous step *produces* rather than
//! something committed. Checking those would need to model Actions'
//! execution order, which is how a guard acquires false positives and then
//! gets deleted.
//!
//! Any value containing `${{` is skipped for the same reason — it is
//! resolved at runtime and this check has no way to evaluate it.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

const WORKFLOW_DIR: &str = ".github/workflows";

/// The keys whose values are real, must-already-exist paths. See the module
/// doc for why the list is this short.
///
/// The regex below is *built from this array* rather than repeating the
/// alternation as a literal. That is deliberate: a hardcoded list duplicated
/// in two places is the exact failure this whole module exists to catch —
/// `release.yml`'s paths drifted from the tree because two copies of one
/// fact were updated separately. Making the constant load-bearing means it
/// cannot be decorative, and `every_checked_key_is_in_the_pattern` cannot
/// pass vacuously.
const CHECKED_KEYS: [&str; 2] = ["dockerfile", "working-directory"];

fn key_pattern() -> Regex {
    let alternation = CHECKED_KEYS.join("|");
    Regex::new(&format!(r"^\s*-?\s*({alternation}):\s*(\S+)\s*$"))
        .expect("CHECKED_KEYS contains no regex metacharacters")
}

pub fn run(root: &Path) -> Result<(), String> {
    let dir = root.join(WORKFLOW_DIR);
    if !dir.is_dir() {
        println!("no {WORKFLOW_DIR} — workflow-path lint vacuously passes");
        return Ok(());
    }

    let re = key_pattern();
    let mut checked = 0usize;
    let mut misses = Vec::new();

    for file in workflow_files(&dir) {
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            let Some(caps) = re.captures(line) else {
                continue;
            };
            let key = &caps[1];
            let value = caps[2].trim_matches(['"', '\'']);
            // Runtime-resolved; nothing static to check.
            if value.contains("${{") {
                continue;
            }
            checked += 1;
            if !root.join(value).exists() {
                misses.push(format!("{rel}:{}: {key}: {value}", index + 1));
            }
        }
    }

    if misses.is_empty() {
        println!("workflow paths OK ({checked} checked across {WORKFLOW_DIR})");
        return Ok(());
    }

    eprintln!("workflow references a path that does not exist:");
    for miss in &misses {
        eprintln!("  {miss}");
    }
    eprintln!();
    eprintln!("`release` runs only on push to main and on v*.*.* tags, so a pull");
    eprintln!("request never executes it — a stale path here is invisible until");
    eprintln!("after it has merged. That is exactly how #271's restructure left");
    eprintln!("every image build broken across four subsequent merges.");
    Err(format!("{} missing workflow path(s)", misses.len()))
}

fn workflow_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_matrix_dockerfile_entry() {
        let re = key_pattern();
        let caps = re
            .captures("            dockerfile: backends/apps/sms-gateway/Dockerfile")
            .expect("should match");
        assert_eq!(&caps[1], "dockerfile");
        assert_eq!(&caps[2], "backends/apps/sms-gateway/Dockerfile");
    }

    #[test]
    fn extracts_a_working_directory_with_a_list_dash() {
        let re = key_pattern();
        let caps = re
            .captures("        - working-directory: sdks/rust/vsms-sdk-rust")
            .expect("should match");
        assert_eq!(&caps[2], "sdks/rust/vsms-sdk-rust");
    }

    /// The keys deliberately left alone. Matching `file:` would fire on
    /// `file: ${{ matrix.dockerfile }}`, and `path:` on artifacts a prior
    /// step produces — both false positives, which is how a guard dies.
    #[test]
    fn ignores_keys_outside_the_checked_set() {
        let re = key_pattern();
        assert!(re
            .captures("          file: ${{ matrix.dockerfile }}")
            .is_none());
        assert!(re.captures("          context: .").is_none());
        assert!(re
            .captures("          path: target/release/sms-gateway")
            .is_none());
        assert!(re.captures("          image: sms-gateway").is_none());
    }

    #[test]
    fn every_checked_key_is_in_the_pattern() {
        let re = key_pattern();
        for key in CHECKED_KEYS {
            assert!(
                re.captures(&format!("  {key}: some/path")).is_some(),
                "{key} is listed in CHECKED_KEYS but the pattern does not match it"
            );
        }
    }
}
