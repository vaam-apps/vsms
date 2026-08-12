//! Every permission this system *enforces* must be *granted* to at least
//! one seeded role — and every permission it grants must be one something
//! actually checks.
//!
//! # Why this exists
//!
//! #211 forwarded a signed-in human's own token upstream for the first
//! time. That immediately surfaced a bug nothing had been able to see
//! before: the seeded roles carried `message:read`/`message:send`, while
//! `rbac::require_permission` checks the literals `sms:read`/`sms:send`.
//! No role granted `dashboard:read` at all. Both were silent, because
//! until #211 no human token had ever reached `require_permission` — the
//! console authenticated as a machine, and machine callers are gated by
//! `scope` rather than by a seeded role's `perms`.
//!
//! The seed was not careless; it was transcribed faithfully from
//! `docs/architecture.md` §5.2's own prose table, which said
//! `message:send`. The *doc* and the *code* had disagreed since long
//! before the seed existed. Writing the prose down as data is what forced
//! the disagreement into the open.
//!
//! That is the whole failure mode: a permission literal is a string in
//! three places — the check, the grant, and the prose — and nothing made
//! them agree. This test makes the first two agree, and fails by name
//! when they stop.
//!
//! # Why a parsing test rather than a live one
//!
//! Deliberately the same two-layer shape as
//! `system_context_golden_list_live_postgres.rs` (#155), but this half
//! needs no database: both sides are literals in tracked files, so a
//! plain `cargo test` can compare them. A live counterpart would only
//! re-prove what the seed already says.
//!
//! # What this cannot catch
//!
//! A permission that is granted *and* checked, but checked on the wrong
//! route, or spelled consistently wrong in both places. This proves the
//! vocabulary agrees with itself, not that it is the right vocabulary —
//! §5.2 remains the source of truth for what each role *should* hold, and
//! that judgement stays human.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/sms-api.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repo root is two levels above crates/sms-api")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "reading {} for the permission-vocabulary check: {error}",
            path.display()
        )
    })
}

/// Every literal passed to `require_permission(ctx, "...")`, across the
/// whole workspace rather than one crate — a procedure in `sms-api` and a
/// future one elsewhere are the same hazard.
fn enforced_by_require_permission() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for relative in tracked_rust_sources() {
        // Production code only. A unit test builds contexts with whatever
        // literals it likes — `rbac.rs`'s own tests use `message:send`
        // precisely because it is *not* a real permission — and counting
        // those would make this check report bugs that do not exist.
        if relative.contains("/tests/") {
            continue;
        }
        let source = read(&relative);
        // Everything from the first `#[cfg(test)]` on is test code too;
        // this codebase keeps unit tests in an inline module at the end of
        // the file, so truncating there is exact rather than approximate.
        let source = source
            .split_once("#[cfg(test)]")
            .map_or(source.as_str(), |(before, _)| before)
            .to_owned();
        let mut rest = source.as_str();
        while let Some(at) = rest.find("require_permission(") {
            rest = &rest[at + "require_permission(".len()..];
            // ..., "permission:literal")  — take the first quoted string
            // in the argument list, which is the literal in every call
            // shape this codebase uses.
            let Some(open) = rest.find('"') else { continue };
            let Some(close) = rest[open + 1..].find('"') else {
                continue;
            };
            let literal = &rest[open + 1..open + 1 + close];
            if literal.contains(':') {
                found.insert(literal.to_owned());
            }
        }
    }
    found
}

/// Every literal in a `RoutePermission { ..., permission: "..." }` —
/// the Tower-layer gates in `router.rs`.
fn enforced_by_route_layers() -> BTreeSet<String> {
    let source = read("crates/sms-api/src/router.rs");
    let mut found = BTreeSet::new();
    let mut rest = source.as_str();
    while let Some(at) = rest.find("permission:") {
        rest = &rest[at + "permission:".len()..];
        let Some(open) = rest.find('"') else { continue };
        let Some(close) = rest[open + 1..].find('"') else {
            continue;
        };
        let literal = &rest[open + 1..open + 1 + close];
        if literal.contains(':') {
            found.insert(literal.to_owned());
        }
    }
    found
}

/// Every permission granted by a seeded role, read from the generated
/// bootstrap migration — the thing that actually reaches a database,
/// rather than §2.10's prose that generates it.
fn granted_by_seeded_roles() -> BTreeSet<String> {
    let sql = read("schema/migrations/postgres/0002_bootstrap/up.sql");
    let start = sql
        .find("INSERT INTO roles")
        .expect("0002_bootstrap must seed the built-in roles — see docs/architecture.md §2.10");
    let statement = &sql[start..sql[start..].find(';').map_or(sql.len(), |e| start + e)];

    let mut found = BTreeSet::new();
    for token in statement.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| c == '\'' || c == ',' || c == ')');
        // Permission literals are the only `word:word` tokens in the seed;
        // labels and descriptions are prose, keys are bare words.
        if cleaned.contains(':') && !cleaned.contains(' ') {
            found.insert(cleaned.to_owned());
        }
    }
    assert!(
        !found.is_empty(),
        "parsed zero permissions out of the roles seed — the parser has drifted from the \
         SQL's shape, which would make every assertion below vacuously true"
    );
    found
}

fn tracked_rust_sources() -> Vec<String> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "*.rs"])
        .current_dir(repo_root())
        .output()
        .expect("git ls-files must run from the repo root");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The one that would have caught #211's bug before it shipped.
#[test]
fn every_enforced_permission_is_granted_to_some_role() {
    let mut enforced = enforced_by_require_permission();
    enforced.extend(enforced_by_route_layers());
    let granted = granted_by_seeded_roles();

    assert!(
        !enforced.is_empty(),
        "found no enforced permission literals at all — the parser has drifted, and this \
         test would pass no matter how broken the vocabulary got"
    );

    let ungranted: Vec<&String> = enforced.difference(&granted).collect();
    assert!(
        ungranted.is_empty(),
        "these permissions are enforced but granted to no seeded role, so every human \
         hitting them is denied regardless of their role: {ungranted:?}\n\n\
         Either add them to the appropriate roles in docs/architecture.md §2.10 (then \
         regenerate 0002_bootstrap with ci/gen-bootstrap-sql.py), or stop checking them.\n\
         Enforced: {enforced:?}\nGranted:  {granted:?}"
    );
}

/// The mirror. A granted permission nothing checks is not a security
/// hole, but it is a lie about what the role table means — the same
/// argument #193 made for not leaving `webhook:manage` unenforced.
///
/// This is a warning, not a failure: §5.2 deliberately grants some
/// permissions ahead of the screens that will check them, and failing the
/// build for that would punish writing the role table honestly. It prints
/// so the drift stays visible.
#[test]
fn granted_permissions_nothing_checks_are_reported() {
    let mut enforced = enforced_by_require_permission();
    enforced.extend(enforced_by_route_layers());
    let granted = granted_by_seeded_roles();

    let unchecked: Vec<&String> = granted.difference(&enforced).collect();
    if !unchecked.is_empty() {
        println!(
            "note: {} granted permission(s) are checked nowhere yet: {unchecked:?}\n\
             That is expected while screens are still being built — §5.2 grants ahead of \
             enforcement on purpose. It becomes a problem only if one is *never* checked, \
             which would make the role table claim a control that does not exist (#193).",
            unchecked.len()
        );
    }
}
