//! `0010_sealed_webhooks` against a database that already holds webhook
//! endpoints with HMAC secrets (VS-8, re-scoped by VS-8R): every existing row
//! becomes an `hmac` endpoint with its secrets exactly as they were and no
//! audience, because its receiver verifies an HMAC today. Nothing is dropped.
//! The database itself then holds the two modes apart: a `sealed` endpoint needs
//! an audience and holds no secret, an `hmac` endpoint needs a secret and names
//! no audience. The audience, when there is one, is unique and never `vsms`, and
//! any number of endpoints may have none. `down.sql` reverses the migration, and refuses only
//! when a `sealed` endpoint exists, which holds no HMAC secret to go back to.
//!
//! Plain `psql`, like the harness's own migration step: no generated client is
//! involved, because the point is the SQL and the "before" state predates the
//! client's columns.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One scratch database per test: the two run in parallel.
const SCRATCH_UP: &str = "vsms_mig0010_up_scratch";
const SCRATCH_DOWN: &str = "vsms_mig0010_down_scratch";

fn migrations_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../migrations/postgres")
        .canonicalize()
        .expect("backends/migrations/postgres")
}

/// `url` with its database replaced by `database`.
fn with_database(url: &str, database: &str) -> String {
    let (head, _) = url.rsplit_once('/').expect("a postgres URL has a path");
    format!("{head}/{database}")
}

/// Runs one `psql` invocation and returns `(success, stdout, stderr)`.
fn psql(url: &str, args: &[&str]) -> (bool, String, String) {
    let output = Command::new("psql")
        .arg(url)
        .args(["-v", "ON_ERROR_STOP=1", "-q", "-At"])
        .args(args)
        .output()
        .expect("spawning `psql` — is the postgresql-client package installed?");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    )
}

fn must(url: &str, args: &[&str]) -> String {
    let (ok, stdout, stderr) = psql(url, args);
    assert!(ok, "psql {args:?} failed: {stderr}");
    stdout
}

fn apply(url: &str, file: &Path) {
    let file = file.to_str().expect("a UTF-8 path");
    must(url, &["-f", file]);
}

fn column_names(url: &str) -> Vec<String> {
    must(
        url,
        &[
            "-c",
            "SELECT column_name FROM information_schema.columns \
             WHERE table_name = 'webhook_endpoints' ORDER BY column_name",
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

/// An insert of one endpoint with exactly the columns named, so each case
/// states what the CHECK it targets looks at and nothing else.
fn insert_endpoint(
    url: &str,
    id: &str,
    mode: &str,
    audience: Option<&str>,
    secret: Option<&str>,
    prev_secret: Option<&str>,
) -> (bool, String, String) {
    insert_endpoint_at(
        url,
        id,
        "https://h.example.test/x",
        mode,
        audience,
        secret,
        prev_secret,
    )
}

/// [`insert_endpoint`] with the endpoint's own URL (`endpoint_url`, the column;
/// `url` is the database being connected to).
fn insert_endpoint_at(
    url: &str,
    id: &str,
    endpoint_url: &str,
    mode: &str,
    audience: Option<&str>,
    secret: Option<&str>,
    prev_secret: Option<&str>,
) -> (bool, String, String) {
    let quote = |value: Option<&str>| value.map_or_else(|| "NULL".to_owned(), |v| format!("'{v}'"));
    let endpoint_url = endpoint_url.replace('\'', "''");
    psql(
        url,
        &[
            "-c",
            &format!(
                "INSERT INTO webhook_endpoints \
                 (id, app_id, url, event_types, delivery_mode, audience, secret, prev_secret, \
                  mask_recipient, max_attempts, version) \
                 VALUES ('{id}', 'app1', '{endpoint_url}', ' ', '{mode}', {}, {}, {}, \
                         true, 8, 1)",
                quote(audience),
                quote(secret),
                quote(prev_secret),
            ),
        ],
    )
}

fn refused_by(result: (bool, String, String), constraint: &str) {
    let (ok, _, stderr) = result;
    assert!(!ok && stderr.contains(constraint), "{constraint}: {stderr}");
}

/// The mode and the columns it decides agree, by the database's own CHECKs.
fn assert_the_modes_are_held_apart(url: &str) {
    // `sealed` needs an audience ...
    refused_by(
        insert_endpoint(url, "m1", "sealed", None, None, None),
        "webhook_endpoints_audience_required_for_sealed_check",
    );
    // ... and holds no secret, current or previous.
    refused_by(
        insert_endpoint(url, "m2", "sealed", Some("recv-m2"), Some("whsec_x"), None),
        "webhook_endpoints_delivery_mode_sealed_holds_no_secret_check",
    );
    refused_by(
        insert_endpoint(
            url,
            "m3",
            "sealed",
            Some("recv-m3"),
            None,
            Some("whsec_old"),
        ),
        "webhook_endpoints_delivery_mode_sealed_holds_no_secret_check",
    );
    // `hmac` needs a secret.
    refused_by(
        insert_endpoint(url, "m4", "hmac", None, None, None),
        "webhook_endpoints_delivery_mode_hmac_needs_secret_check",
    );
    // ... and an `hmac` endpoint names no audience, so it cannot squat the name
    // a real `sealed` endpoint may need (the audience is unique across both modes).
    refused_by(
        insert_endpoint(url, "m9", "hmac", Some("recv-m9"), Some("whsec_m9"), None),
        "webhook_endpoints_audience_only_for_sealed_check",
    );
    // Only the two modes exist.
    refused_by(
        insert_endpoint(url, "m5", "plain", None, Some("whsec_x"), None),
        "webhook_endpoints_delivery_mode_enum_check",
    );
    // The legitimate shapes go in: a sealed one with an audience and no secret,
    // and any number of `hmac` ones with a secret and no audience (NULLs do not
    // collide in the unique index).
    for (id, result) in [
        (
            "m6",
            insert_endpoint(url, "m6", "sealed", Some("recv-m6"), None, None),
        ),
        (
            "m7",
            insert_endpoint(url, "m7", "hmac", None, Some("whsec_m7"), None),
        ),
        (
            "m8",
            insert_endpoint(url, "m8", "hmac", None, Some("whsec_m8"), Some("whsec_old")),
        ),
    ] {
        assert!(result.0, "{id}: {}", result.2);
    }
}

/// The one rule about `url` holds a `sealed` endpoint alone: a sealed delivery
/// binds the URL's path as its route. An `hmac` endpoint keeps any URL, a query
/// string included, and the migration's seed already holds such rows.
fn assert_the_url_rule_is_a_sealed_endpoints_alone(url: &str) {
    const CHECK: &str = "webhook_endpoints_url_sealable_for_sealed_check";
    let unsealable = [
        "https://h.example.test/hook?token=abc",
        "https://h.example.test/hook?",
        "https://h.example.test/hook#frag",
        "https://u:pw@h.example.test/hook",
        "https://h.example.test/hooks/{tenant}",
        "https://h.example.test/a%2Fb",
        "ftp://h.example.test/hook",
        "https://exämple.test/hook",
        "https://under_score.example.test/hook",
        "https://h.example.test:0/hook",
        "https://h.example.test:65536/hook",
    ];
    for (i, endpoint_url) in unsealable.iter().enumerate() {
        refused_by(
            insert_endpoint_at(
                url,
                &format!("u{i}"),
                endpoint_url,
                "sealed",
                Some(&format!("recv-u{i}")),
                None,
                None,
            ),
            CHECK,
        );
        let (ok, _, stderr) = insert_endpoint_at(
            url,
            &format!("h{i}"),
            endpoint_url,
            "hmac",
            None,
            Some("whsec_url_rule"),
            None,
        );
        assert!(ok, "an hmac endpoint keeps {endpoint_url}: {stderr}");
    }
    for (i, endpoint_url) in [
        "https://h.example.test/webhooks/vsms",
        "HTTP://H.example.test:8080",
        "https://[::1]:8443/hook",
        "https://h.example.test/a,b;c=d@e:f'g",
    ]
    .iter()
    .enumerate()
    {
        let (ok, _, stderr) = insert_endpoint_at(
            url,
            &format!("s{i}"),
            endpoint_url,
            "sealed",
            Some(&format!("recv-s{i}")),
            None,
            None,
        );
        assert!(ok, "a sealed endpoint at {endpoint_url}: {stderr}");
    }
}

/// The CHECK carries `sms_webhook::SEALABLE_URL_PATTERN` and nothing else: the
/// Rust constant the corpus test holds narrower than `delivery_route` is the
/// pattern the database applies.
#[test]
fn the_migration_carries_the_pattern_the_rust_constant_names() {
    let up = std::fs::read_to_string(migrations_root().join("0010_sealed_webhooks/up.sql"))
        .expect("0010/up.sql");
    let statement = up
        .lines()
        .find(|line| {
            line.starts_with("ALTER TABLE")
                && line.contains("webhook_endpoints_url_sealable_for_sealed_check")
        })
        .expect("0010 adds webhook_endpoints_url_sealable_for_sealed_check");
    let expected = format!(
        "ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_url_sealable_for_sealed_check \
         CHECK (delivery_mode <> 'sealed' OR url ~ '{}');",
        sms_webhook::SEALABLE_URL_PATTERN.replace('\'', "''")
    );
    assert_eq!(statement, expected);
}

/// Two receivers cannot share a name (the gateway has one signing key for every
/// app, so the name is what tells their seals apart), and `vsms` is the name the
/// gateway's own routes answer to. The length CHECK is the generator's.
fn assert_audience_is_unique_and_never_vsms(url: &str) {
    refused_by(
        insert_endpoint(url, "a1", "sealed", Some("recv-m6"), None, None),
        "webhook_endpoints_audience_key",
    );
    refused_by(
        insert_endpoint(url, "a2", "sealed", Some("vsms"), None, None),
        "webhook_endpoints_audience_not_vsms_check",
    );
    refused_by(
        insert_endpoint(url, "a3", "sealed", Some(""), None, None),
        "webhook_endpoints_audience_length_check",
    );
    // Only the exact name: `vsms-receiver` and `Vsms` are other names.
    for (id, audience) in [("a4", "vsms-receiver"), ("a5", "Vsms")] {
        let (ok, _, stderr) = insert_endpoint(url, id, "sealed", Some(audience), None, None);
        assert!(ok, "{audience}: {stderr}");
    }
}

fn seed_legacy_endpoints(url: &str) {
    must(
        url,
        &[
            "-c",
            "INSERT INTO apps (id, name, slug, monthly_quota, ip_allowlist, transliterate_to_gsm7, version) \
             VALUES ('app1', 'legacy app', 'legacy-app', 1000, ' ', false, 1)",
        ],
    );
    // Endpoints as 0001_init allowed them: one rotated with an overlap secret,
    // the rest with a secret alone, and URLs of every shape (the migration
    // must not look at the URL).
    must(
        url,
        &[
            "-c",
            "INSERT INTO webhook_endpoints \
             (id, app_id, url, event_types, secret, prev_secret, secret_rotated_at, mask_recipient, max_attempts, version) \
             VALUES \
             ('ep1', 'app1', 'https://hooks.example.test:8443/webhooks/vsms?x=1', ' message.delivered ', \
              'sms_webhook_current', 'sms_webhook_previous', '2026-03-01T00:00:00Z', true, 8, 1), \
             ('ep2', 'app1', 'http://plain.example.test/hook', ' message.delivered ', \
              'sms_webhook_other', NULL, NULL, false, 8, 1), \
             ('ep3', 'app1', 'https://u:p@ss@raw-at.example.test/x', ' message.delivered ', \
              'sms_webhook_rawat', NULL, NULL, false, 8, 1), \
             ('ep4', 'app1', 'https://creds.example.test/other', ' message.delivered ', \
              'sms_webhook_creds', NULL, NULL, false, 8, 1), \
             ('ep5', 'app1', 'https://creds.example.test/other', ' message.delivered ', \
              'sms_webhook_creds2', NULL, NULL, false, 8, 1)",
        ],
    );
}

fn migration_dirs() -> (Vec<PathBuf>, PathBuf) {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(migrations_root())
        .expect("reading the migrations")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    let (before, from_0010): (Vec<_>, Vec<_>) = dirs.into_iter().partition(|dir| {
        dir.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name < "0010")
    });
    assert_eq!(
        from_0010.len(),
        1,
        "0010 is the newest migration: later ones need a case here"
    );
    (before, from_0010.into_iter().next().expect("0010"))
}

fn recreate_scratch(admin: &str, name: &str) {
    drop_scratch(admin, name);
    must(admin, &["-c", &format!("CREATE DATABASE {name}")]);
}

fn drop_scratch(admin: &str, name: &str) {
    must(
        admin,
        &[
            "-c",
            &format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        ],
    );
}

fn apply_before_0010(scratch: &str, before: &[PathBuf]) {
    for dir in before {
        for file in ["up.pre.sql", "up.sql"] {
            let path = dir.join(file);
            if path.exists() {
                apply(scratch, &path);
            }
        }
    }
}

const SECRETS: &str = "SELECT id, secret, coalesce(prev_secret, '-'), \
                       coalesce(secret_rotated_at::text, '-') FROM webhook_endpoints ORDER BY id";

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn legacy_endpoints_become_hmac_with_their_secrets_and_no_audience() {
    let base = sms_test_support::database_url().await;
    let admin = with_database(&base, "postgres");
    let scratch = with_database(&base, SCRATCH_UP);
    recreate_scratch(&admin, SCRATCH_UP);
    let (before, from_0010) = migration_dirs();
    apply_before_0010(&scratch, &before);

    let legacy = column_names(&scratch);
    for column in ["secret", "prev_secret", "secret_rotated_at"] {
        assert!(legacy.contains(&column.to_owned()), "{column} before 0010");
    }
    assert!(!legacy.contains(&"audience".to_owned()));
    assert!(!legacy.contains(&"delivery_mode".to_owned()));

    seed_legacy_endpoints(&scratch);
    let secrets_before = must(&scratch, &["-c", SECRETS]);

    apply(&scratch, &from_0010.join("up.sql"));

    // Nothing was dropped: every column of before is still there, and the two
    // new ones are.
    let after = column_names(&scratch);
    for column in &legacy {
        assert!(after.contains(column), "{column} did not survive 0010");
    }
    for column in ["audience", "delivery_mode"] {
        assert!(after.contains(&column.to_owned()), "{column} after 0010");
    }

    // Every existing row is an `hmac` endpoint with the secrets it had, byte for
    // byte (rotation timestamp included), and no audience: a guessed one would
    // occupy a name a real endpoint may need.
    assert_eq!(must(&scratch, &["-c", SECRETS]), secrets_before);
    assert_eq!(
        must(
            &scratch,
            &[
                "-c",
                "SELECT delivery_mode, count(*), count(audience) FROM webhook_endpoints GROUP BY 1"
            ],
        ),
        "hmac|5|0"
    );

    // `secret` is optional now (a sealed endpoint has none) and the rest of the
    // table kept its shape.
    assert_eq!(
        must(
            &scratch,
            &[
                "-c",
                "SELECT is_nullable FROM information_schema.columns \
                     WHERE table_name = 'webhook_endpoints' AND column_name = 'secret'"
            ],
        ),
        "YES"
    );
    assert_eq!(
        must(
            &scratch,
            &[
                "-c",
                "SELECT is_nullable FROM information_schema.columns \
                     WHERE table_name = 'webhook_endpoints' AND column_name = 'delivery_mode'"
            ],
        ),
        "NO"
    );

    assert_the_modes_are_held_apart(&scratch);
    assert_audience_is_unique_and_never_vsms(&scratch);
    assert_the_url_rule_is_a_sealed_endpoints_alone(&scratch);

    drop_scratch(&admin, SCRATCH_UP);
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_rollback_restores_the_legacy_shape_and_refuses_only_for_a_sealed_endpoint() {
    let base = sms_test_support::database_url().await;
    let admin = with_database(&base, "postgres");
    let scratch = with_database(&base, SCRATCH_DOWN);
    recreate_scratch(&admin, SCRATCH_DOWN);
    let (before, from_0010) = migration_dirs();
    apply_before_0010(&scratch, &before);
    seed_legacy_endpoints(&scratch);
    let secrets_before = must(&scratch, &["-c", SECRETS]);
    let columns_before = column_names(&scratch);

    apply(&scratch, &from_0010.join("up.sql"));
    let down = from_0010.join("down.sql");
    let down = down.to_str().expect("a path");

    // A sealed endpoint holds no HMAC secret to go back to: the rollback says so
    // and changes nothing.
    must(
        &scratch,
        &[
            "-c",
            "INSERT INTO webhook_endpoints \
        (id, app_id, url, event_types, delivery_mode, audience, mask_recipient, max_attempts, version) \
        VALUES ('sealed1', 'app1', 'https://s.example.test/x', ' ', 'sealed', 'recv-s1', true, 8, 1)",
        ],
    );
    let (ok, _, stderr) = psql(&scratch, &["-f", down]);
    assert!(
        !ok && stderr.contains("1 sealed webhook endpoint"),
        "{stderr}"
    );
    assert!(column_names(&scratch).contains(&"delivery_mode".to_owned()));

    // Gone, the rollback goes through, and the table is what 0001_init made it,
    // secrets untouched.
    must(
        &scratch,
        &["-c", "DELETE FROM webhook_endpoints WHERE id = 'sealed1'"],
    );
    must(&scratch, &["-f", down]);
    assert_eq!(column_names(&scratch), columns_before);
    assert_eq!(must(&scratch, &["-c", SECRETS]), secrets_before);
    assert_eq!(
        must(
            &scratch,
            &[
                "-c",
                "SELECT is_nullable FROM information_schema.columns \
                     WHERE table_name = 'webhook_endpoints' AND column_name = 'secret'"
            ],
        ),
        "NO"
    );
    assert_eq!(
        must(
            &scratch,
            &[
                "-c",
                "SELECT count(*) FROM pg_constraint \
                     WHERE conrelid = 'webhook_endpoints'::regclass AND contype = 'c'"
            ],
        ),
        "0"
    );

    // Forward again: the migration is repeatable over what the rollback left.
    // Then a rollback is one transaction. Make its middle fail (a constraint it
    // drops by name is gone) and run it the way `psql -f` runs a file by
    // default, carrying on past an error: the statements before the failure are
    // undone with it, not left half applied.
    apply(&scratch, &from_0010.join("up.sql"));
    must(
        &scratch,
        &[
            "-c",
            "ALTER TABLE webhook_endpoints \
             DROP CONSTRAINT webhook_endpoints_delivery_mode_hmac_needs_secret_check",
        ],
    );
    let constraints = "SELECT count(*) FROM pg_constraint \
                       WHERE conrelid = 'webhook_endpoints'::regclass AND contype = 'c'";
    let constraints_before = must(&scratch, &["-c", constraints]);
    let output = Command::new("psql")
        .arg(&scratch)
        .args(["-q", "-f", down])
        .output()
        .expect("spawning psql");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist"),
        "the rollback must fail: {stderr}"
    );
    assert_eq!(must(&scratch, &["-c", constraints]), constraints_before);
    assert!(column_names(&scratch).contains(&"delivery_mode".to_owned()));
    assert_eq!(
        must(
            &scratch,
            &[
                "-c",
                "SELECT count(*) FROM pg_indexes WHERE indexname = 'webhook_endpoints_audience_key'"
            ],
        ),
        "1"
    );
    must(
        &scratch,
        &[
            "-c",
            "ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_delivery_mode_hmac_needs_secret_check \
             CHECK (delivery_mode <> 'hmac' OR secret IS NOT NULL)",
        ],
    );

    // The migration was repeatable over what the first rollback left (the
    // `apply` above), and the secrets are what they were.
    assert_eq!(must(&scratch, &["-c", SECRETS]), secrets_before);

    drop_scratch(&admin, SCRATCH_DOWN);
}
