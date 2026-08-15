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
//! A path reference is the part of such a file that *can* be verified
//! statically, cheaply, from any branch. So it is.
//!
//! # The second instance, which this check's first version missed
//!
//! The first version scanned `.github/workflows/**` and nothing else.
//! `compose.dev.yaml` — what `just demo` runs, the primary local
//! development entry point — carried **eleven** stale `app/*` and `admin/`
//! Dockerfile paths from the same #271 restructure, and stayed broken
//! through seven further merges. It surfaced only when someone actually
//! ran `just demo` and got `lstat /Users/.../app: no such file or
//! directory`.
//!
//! The lesson is not "add compose files"; it is that the blind spot was
//! *the scan root*, not the rule. A build-file path reference is
//! statically checkable wherever it lives, so the roots below cover every
//! place this repo declares one. `deploy/docker-compose.yml` was already
//! correct — #271 updated it — which is what made the breakage partial and
//! easy to miss, exactly as the `demo` matrix entry did for `release.yml`.
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

/// Every place this repo declares a build-file path. Not just workflows:
/// see the module doc for why limiting the scan root was the actual bug.
const SCAN_ROOTS: [&str; 2] = [".github/workflows", "deploy"];

/// Compose files at the repo root, which is where `just demo`'s own
/// `compose.dev.yaml` lives.
const ROOT_FILES: [&str; 3] = ["compose.yml", "compose.dev.yaml", "compose.demo.yaml"];

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
    let mut files: Vec<PathBuf> = Vec::new();
    for r in SCAN_ROOTS {
        let dir = root.join(r);
        if dir.is_dir() {
            files.extend(yaml_files(&dir));
        }
    }
    for f in ROOT_FILES {
        let p = root.join(f);
        if p.is_file() {
            files.push(p);
        }
    }
    if files.is_empty() {
        println!("no workflow or compose files — path lint vacuously passes");
        return Ok(());
    }

    let re = key_pattern();
    let mut checked = 0usize;
    let mut misses = Vec::new();

    for file in files {
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
        println!("build-file paths OK ({checked} checked)");
        return Ok(());
    }

    eprintln!("a workflow or compose file references a path that does not exist:");
    for miss in &misses {
        eprintln!("  {miss}");
    }
    eprintln!();
    eprintln!("Nothing in CI runs `release` on a pull request, and nothing runs");
    eprintln!("`just demo` at all — so a stale path in either is invisible until");
    eprintln!("after it merges. #271's restructure broke every image build for");
    eprintln!("four merges that way, and `just demo` for seven.");
    Err(format!("{} missing path(s)", misses.len()))
}

fn yaml_files(dir: &Path) -> Vec<PathBuf> {
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
        assert!(
            re.captures("          file: ${{ matrix.dockerfile }}")
                .is_none()
        );
        assert!(re.captures("          context: .").is_none());
        assert!(
            re.captures("          path: target/release/sms-gateway")
                .is_none()
        );
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
