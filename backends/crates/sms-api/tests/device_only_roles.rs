//! Every role the schema names must be decided about: device-only, or on a named
//! allow-list with a reason (VSA-4b's tripwire).
//!
//! # Why this exists
//!
//! A person's token stands as the person's role, except that
//! [`sms_api::DEVICE_ONLY_ROLES`] (`owner`, `admin`) are refused behind a token.
//! That list is a claim about which roles are powerful enough to need a device.
//! It stays true only while nobody adds a role with the same power under a new
//! name: a `hasRole('superadmin')` on `User.update` would be an owner-class role a
//! password session carries, and nothing else would notice. So every
//! `hasRole('...')` literal in `schemas/vsms.cstack`, and every role key the
//! bootstrap seeds, must be in `DEVICE_ONLY_ROLES` or in [`ALLOWED`] below, each
//! with the reason it may act from a token. A new literal fails here by name and
//! whoever adds it has to make that decision in the diff.
//!
//! It reads the schema and the seed as text, like `permission_vocabulary.rs`: no
//! database, so plain `cargo test` runs it.
//!
//! # What this cannot catch
//!
//! A role on the allow-list that is later given more power in the schema. The
//! reason beside it is a claim a reviewer has to keep true.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Roles a person's token may carry, or that no person's token can carry at all,
/// each with why.
const ALLOWED: &[(&str, &str)] = &[
    (
        "operator",
        "runs traffic: sends, routes and providers; cannot create users, clients or keys",
    ),
    (
        "developer",
        "integrates apps: reads apps and manages webhooks; no user or key administration",
    ),
    (
        "support",
        "first line: reads messages and manages opt-outs; no write on identity or routing",
    ),
    (
        "auditor",
        "read-only oversight, including the audit log; no mutation",
    ),
    (
        "system",
        "the synthetic internal role: refused for every person by `refuse_reserved_role` and a CHECK",
    ),
    (
        "app",
        "a machine client's sentinel role, matched by no `hasRole`; refused for every person likewise",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repo root is three levels above backends/crates/sms-api")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// Every `hasRole('x')` literal in `text`, by name.
fn has_role_literals(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(at) = rest.find("hasRole(") {
        rest = &rest[at + "hasRole(".len()..];
        let Some(open) = rest.chars().next().filter(|c| *c == '\'' || *c == '"') else {
            continue;
        };
        if let Some(len) = rest[1..].find(open) {
            found.insert(rest[1..=len].to_owned());
        }
    }
    found
}

/// The keys of the roles the bootstrap migration seeds.
fn seeded_role_keys(sql: &str) -> BTreeSet<String> {
    let start = sql
        .find("INSERT INTO roles")
        .expect("the bootstrap seeds the roles");
    let block = &sql[start..];
    let end = block.find(';').expect("the INSERT ends");
    block[..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let line = line.strip_prefix('(')?;
            let line = line.strip_prefix('\'')?;
            line.split('\'').next().map(str::to_owned)
        })
        .collect()
}

/// The roles with a decision: device-only, or allowed with a reason.
fn decided() -> BTreeSet<String> {
    sms_api::DEVICE_ONLY_ROLES
        .iter()
        .map(|role| (*role).to_owned())
        .chain(ALLOWED.iter().map(|(role, _)| (*role).to_owned()))
        .collect()
}

fn assert_all_decided(source: &str, found: &BTreeSet<String>) {
    let decided = decided();
    let undecided: Vec<&String> = found.difference(&decided).collect();
    assert!(
        undecided.is_empty(),
        "{source} names the role(s) {undecided:?}, which are neither in \
         `sms_api::DEVICE_ONLY_ROLES` (owner-class: refused behind a person's token) nor in \
         `ALLOWED` in this file with the reason they may act from a token. Decide which, in \
         the same change."
    );
}

#[test]
fn every_role_literal_in_the_schema_is_decided() {
    let found = has_role_literals(&read("schemas/vsms.cstack"));
    assert!(
        found.len() >= 6,
        "the scan found only {found:?}: it has stopped reading the schema"
    );
    assert_all_decided("schemas/vsms.cstack", &found);
}

#[test]
fn every_seeded_role_is_decided() {
    let keys = seeded_role_keys(&read("backends/migrations/postgres/0002_bootstrap/up.sql"));
    assert!(
        keys.contains("owner") && keys.contains("operator"),
        "the scan found only {keys:?}: it has stopped reading the seed"
    );
    assert_all_decided("the 0002_bootstrap role seed", &keys);
}

/// The device-only list is exactly the owner-class pair, each of which the schema
/// really does name, and none is also on the allow-list.
#[test]
fn the_device_only_roles_are_owner_and_admin_and_are_not_also_allowed() {
    assert_eq!(sms_api::DEVICE_ONLY_ROLES, ["owner", "admin"]);
    let found = has_role_literals(&read("schemas/vsms.cstack"));
    for role in sms_api::DEVICE_ONLY_ROLES {
        assert!(found.contains(*role), "{role} is not in the schema");
        assert!(
            ALLOWED.iter().all(|(allowed, _)| allowed != role),
            "{role} is both device-only and allowed"
        );
    }
    for (role, reason) in ALLOWED {
        assert!(!reason.trim().is_empty(), "{role} has no reason");
    }
}

/// The scan itself: a literal it should find, one in either quote style, and a
/// new owner-class role it must surface as undecided.
#[test]
fn the_scan_finds_a_new_role_literal() {
    let schema = r#"
        @@allow("update", hasRole('owner') || hasRole("admin") || hasRole('superadmin'))
    "#;
    let found = has_role_literals(schema);
    assert_eq!(
        found.iter().map(String::as_str).collect::<Vec<_>>(),
        ["admin", "owner", "superadmin"]
    );
    let decided = decided();
    let undecided: Vec<&String> = found.difference(&decided).collect();
    assert_eq!(undecided, [&"superadmin".to_owned()]);
    let seed = "INSERT INTO roles (key) VALUES\n    ('owner',   'x'),\n    ('root',    'y');";
    assert_eq!(
        seeded_role_keys(seed).into_iter().collect::<Vec<_>>(),
        ["owner", "root"]
    );
}
