//! R1 — all data access goes through `CrateStack` delegates. Never raw `sqlx`.
//!
//! Port of the deleted `ci/assert-no-raw-sqlx.sh`. The allowlist below is
//! the enforced list of named exceptions; `CONTRIBUTING.md`'s own R1
//! exceptions table is supposed to be the human-readable mirror of it.
//!
//! Porting this surfaced one real disagreement between the two, not
//! invented for this PR: the shipped script's allowlist has always
//! included `backends/crates/sms-test-support/src/lib.rs` (it takes
//! `pg_advisory_lock`/`pg_advisory_unlock` and issues `CREATE DATABASE`/
//! `DROP DATABASE` against the test-harness's own per-binary scratch
//! databases — advisory locks and DDL, the same two categories two
//! existing rows already cover, just never attached to this file), but
//! `CONTRIBUTING.md`'s table never listed it. That gap predates this PR;
//! fixed here by adding the row (see `CONTRIBUTING.md`) rather than
//! silently carrying forward an enforced exception nobody could read the
//! reasoning for.
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// The two source roots. Both must be scanned — `backends/crates/` is libraries,
/// `backends/apps/` is binaries — or the other stays free to reach past the
/// delegates.
const ROOTS: [&str; 2] = ["backends/crates", "backends/apps"];

/// Every exception, by the suffix of its path relative to the repo root.
/// Matched with `ends_with`, mirroring the original script's `grep -vE`
/// alternation over path fragments like `sms-worker/src/lease\.rs`. Keep
/// this in lockstep with `CONTRIBUTING.md`'s own R1 exceptions table —
/// that file names the reasoning, this one is the enforced list.
const ALLOWLIST: [&str; 12] = [
    "sms-worker/src/lease.rs",
    "sms-worker/src/notify.rs",
    "sms-worker/src/drain.rs",
    "sms-worker/src/jobs/reap_outbox.rs",
    "sms-worker/tests/anchor_audit_live_postgres.rs",
    "sms-api/src/cache.rs",
    "sms-api/src/worker_locks.rs",
    "sms-api/src/audit_log.rs",
    "sms-test-support/src/lib.rs",
    "sms-gateway/src/health.rs",
    "sms-migrate/src/main.rs",
    "sms-gateway/tests/login_flow_live_postgres.rs",
];

/// Same pattern as the original `grep -E 'sqlx::(query|query_as|query_scalar|raw_sql)\b'`:
/// a real word boundary after the alternation, so `sqlx::query` does not
/// also match inside `sqlx::query_as`/`sqlx::query_scalar` (there is no
/// word boundary between `query` and the `_` that follows), while a bare
/// macro call like `sqlx::query_scalar!(...)` still matches (`!` is not a
/// word character).
fn pattern() -> Regex {
    Regex::new(r"sqlx::(query_as|query_scalar|query|raw_sql)\b")
        .expect("pattern is a fixed, valid regex")
}

pub fn run(root: &Path) -> Result<(), String> {
    let roots: Vec<PathBuf> = ROOTS
        .iter()
        .map(|r| root.join(r))
        .filter(|p| p.is_dir())
        .collect();

    if roots.is_empty() {
        println!("no backends/crates/ or backends/apps/ yet — R1 lint vacuously passes");
        return Ok(());
    }

    let re = pattern();
    let mut hits = Vec::new();
    for r in &roots {
        for file in rust_sources(r) {
            let rel = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            if ALLOWLIST.iter().any(|a| rel.ends_with(a)) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            for (lineno, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(format!("{rel}:{}:{line}", lineno + 1));
                }
            }
        }
    }

    if hits.is_empty() {
        println!("R1 OK (scanned: {})", ROOTS.join(" "));
        return Ok(());
    }

    eprintln!("R1 violation — raw sqlx outside the named exceptions:");
    for hit in &hits {
        eprintln!("{hit}");
    }
    eprintln!();
    eprintln!("See CONTRIBUTING.md. Raw SQL bypasses row-level policy, audit rows,");
    eprintln!("@@emit outbox rows and version bumping — all four, silently.");
    Err("R1 violation".to_owned())
}

/// Every `*.rs` file under `dir`, recursively — a plain walk is sufficient
/// here: neither `backends/crates/` nor `backends/apps/` has a nested `target/`
/// directory in this workspace (a single shared `target/` sits at the repo root),
/// so there is nothing to exclude that `grep -r` would not also have scanned.
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
            out.extend(rust_sources(&path));
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
    fn matches_every_alternative_with_a_boundary() {
        let re = pattern();
        assert!(re.is_match("sqlx::query(\"select 1\")"));
        assert!(re.is_match("sqlx::query_as::<_, Row>(\"select 1\")"));
        assert!(re.is_match("sqlx::query_scalar!(\"select 1\")"));
        assert!(re.is_match("sqlx::raw_sql(\"select 1\").execute(&pool)"));
    }

    #[test]
    fn does_not_match_unrelated_identifiers() {
        let re = pattern();
        assert!(!re.is_match("cratestack::sqlx::postgres::PgPoolOptions"));
        assert!(!re.is_match("use cratestack::sqlx::{query, query_scalar};"));
    }

    #[test]
    fn allowlisted_paths_are_suffix_matched() {
        let rel = "backends/crates/sms-worker/src/lease.rs";
        assert!(ALLOWLIST.iter().any(|a| rel.ends_with(a)));
        let rel = "backends/apps/sms-gateway/src/health.rs";
        assert!(ALLOWLIST.iter().any(|a| rel.ends_with(a)));
        let rel = "backends/crates/sms-api/src/procedures.rs";
        assert!(!ALLOWLIST.iter().any(|a| rel.ends_with(a)));
    }
}
