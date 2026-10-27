//! `0010_sealed_webhooks` against a database that already holds a webhook
//! endpoint with the HMAC secrets VS-8 deletes: the migration keeps the row,
//! gives it an `audience` (the URL's origin, lower-cased, without credentials,
//! and distinct even when two rows share one), and the three secret columns are
//! gone afterwards. The audience is unique and never `vsms`, which the database
//! itself enforces. Its `down.sql` refuses: a secret that was deleted on
//! purpose must not come back from a rollback.
//!
//! Plain `psql`, like the harness's own migration step: no generated client is
//! involved, because the point is the SQL and the client no longer has the
//! columns to write the "before" state with.

use std::path::{Path, PathBuf};
use std::process::Command;

const SCRATCH: &str = "vsms_mig0010_scratch";

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

/// NOT NULL and non-empty are the database's own refusals, not the API's.
fn assert_audience_is_required(url: &str) {
    let (ok, _, stderr) = psql(
        url,
        &[
            "-c",
            "INSERT INTO webhook_endpoints \
             (id, app_id, url, event_types, mask_recipient, max_attempts, version) \
             VALUES ('ep3', 'app1', 'https://h.example.test/x', ' ', true, 8, 1)",
        ],
    );
    assert!(!ok && stderr.contains("audience"), "{stderr}");
    let (ok, _, stderr) = psql(
        url,
        &[
            "-c",
            "INSERT INTO webhook_endpoints \
             (id, app_id, url, event_types, audience, mask_recipient, max_attempts, version) \
             VALUES ('ep4', 'app1', 'https://h.example.test/x', ' ', '', true, 8, 1)",
        ],
    );
    assert!(
        !ok && stderr.contains("webhook_endpoints_audience_length_check"),
        "{stderr}"
    );
}

fn insert_with_audience(url: &str, id: &str, audience: &str) -> (bool, String, String) {
    psql(
        url,
        &[
            "-c",
            &format!(
                "INSERT INTO webhook_endpoints \
                 (id, app_id, url, event_types, audience, mask_recipient, max_attempts, version) \
                 VALUES ('{id}', 'app1', 'https://h.example.test/x', ' ', '{audience}', true, 8, 1)"
            ),
        ],
    )
}

/// Two receivers cannot share a name (the gateway has one signing key for every
/// app, so the name is what tells their seals apart), and `vsms` is the name the
/// gateway's own routes answer to.
fn assert_audience_is_unique_and_never_vsms(url: &str) {
    let (ok, _, stderr) = insert_with_audience(url, "ep10", "https://hooks.example.test:8443");
    assert!(
        !ok && stderr.contains("webhook_endpoints_audience_key"),
        "{stderr}"
    );
    let (ok, _, stderr) = insert_with_audience(url, "ep11", "vsms");
    assert!(
        !ok && stderr.contains("webhook_endpoints_audience_not_vsms_check"),
        "{stderr}"
    );
    // Only the exact name: `vsms-receiver` and `Vsms` are other names.
    for (id, audience) in [("ep12", "vsms-receiver"), ("ep13", "Vsms")] {
        let (ok, _, stderr) = insert_with_audience(url, id, audience);
        assert!(ok, "{audience}: {stderr}");
    }
}

fn seed_legacy_endpoints(url: &str) {
    // Two endpoints that carry live HMAC secrets.
    must(
        url,
        &[
            "-c",
            "INSERT INTO apps (id, name, slug, monthly_quota, ip_allowlist, transliterate_to_gsm7, version) \
             VALUES ('app1', 'legacy app', 'legacy-app', 1000, ' ', false, 1)",
        ],
    );
    must(
        url,
        &[
            "-c",
            "INSERT INTO webhook_endpoints \
             (id, app_id, url, event_types, secret, prev_secret, secret_rotated_at, mask_recipient, max_attempts, version) \
             VALUES \
             ('ep1', 'app1', 'https://hooks.example.test:8443/webhooks/vsms?x=1', ' message.delivered ', \
              'sms_webhook_current', 'sms_webhook_previous', now(), true, 8, 1), \
             ('ep2', 'app1', 'http://plain.example.test/hook', ' message.delivered ', \
              'sms_webhook_other', NULL, NULL, false, 8, 1), \
             ('ep5', 'app1', 'HTTPS://Upper.Example.TEST/hook', ' message.delivered ', \
              'sms_webhook_upper', NULL, NULL, false, 8, 1), \
             ('ep8', 'app1', 'ftp://u:pw@files.example.test/x', ' message.delivered ', \
              'sms_webhook_ftp', NULL, NULL, false, 8, 1), \
             ('ep9', 'app1', 'not-a-url-u@pw', ' message.delivered ', \
              'sms_webhook_none', NULL, NULL, false, 8, 1)",
        ],
    );
    // A URL that carries a credential, and a second endpoint at the same origin.
    // `ep7` is the older of the two, so it keeps the bare origin although its id
    // sorts after `ep6`'s.
    must(
        url,
        &[
            "-c",
            "INSERT INTO webhook_endpoints \
             (id, app_id, url, event_types, secret, created_at, mask_recipient, max_attempts, version) \
             VALUES \
             ('ep6', 'app1', 'https://deploy:s3cret@creds.example.test/hook', ' message.delivered ', \
              'sms_webhook_creds', '2026-02-01T00:00:00Z', false, 8, 1), \
             ('ep7', 'app1', 'https://creds.example.test/other', ' message.delivered ', \
              'sms_webhook_creds2', '2026-01-01T00:00:00Z', false, 8, 1)",
        ],
    );
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_migration_keeps_an_endpoint_gives_it_an_audience_and_drops_the_secrets() {
    let base = sms_test_support::database_url().await;
    let admin = with_database(&base, "postgres");
    let scratch = with_database(&base, SCRATCH);

    must(
        &admin,
        &[
            "-c",
            &format!("DROP DATABASE IF EXISTS {SCRATCH} WITH (FORCE)"),
        ],
    );
    must(&admin, &["-c", &format!("CREATE DATABASE {SCRATCH}")]);

    // Everything up to, but not including, 0010: what a deployed database holds.
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
    for dir in &before {
        for file in ["up.pre.sql", "up.sql"] {
            let path = dir.join(file);
            if path.exists() {
                apply(&scratch, &path);
            }
        }
    }

    let legacy = column_names(&scratch);
    for column in ["secret", "prev_secret", "secret_rotated_at"] {
        assert!(legacy.contains(&column.to_owned()), "{column} before 0010");
    }
    assert!(!legacy.contains(&"audience".to_owned()));

    seed_legacy_endpoints(&scratch);

    apply(&scratch, &from_0010[0].join("up.sql"));

    let after = column_names(&scratch);
    for column in ["secret", "prev_secret", "secret_rotated_at"] {
        assert!(
            !after.contains(&column.to_owned()),
            "{column} survived 0010"
        );
    }
    assert!(after.contains(&"audience".to_owned()));

    // The rows survive, addressed to their URL's origin until an operator names
    // the receiver's real audience: the scheme and host in lower case whatever
    // the URL spelled, no credential, the older of two rows at one origin
    // keeping it and the newer one told apart by its id, a URL with no origin
    // never copied.
    let rows = must(
        &scratch,
        &[
            "-c",
            "SELECT id, audience FROM webhook_endpoints ORDER BY id",
        ],
    );
    assert_eq!(
        rows,
        "ep1|https://hooks.example.test:8443\n\
         ep2|http://plain.example.test\n\
         ep5|https://upper.example.test\n\
         ep6|https://creds.example.test#ep6\n\
         ep7|https://creds.example.test\n\
         ep8|ftp://files.example.test\n\
         ep9|endpoint-ep9"
    );
    let leaked = must(
        &scratch,
        &[
            "-c",
            "SELECT count(*) FROM webhook_endpoints \
             WHERE audience LIKE '%s3cret%' OR audience LIKE '%deploy%' OR audience LIKE '%pw%'",
        ],
    );
    assert_eq!(leaked, "0", "a credential was copied into an audience");

    assert_audience_is_required(&scratch);
    assert_audience_is_unique_and_never_vsms(&scratch);

    // Forward-only: the rollback refuses instead of restoring empty secrets.
    let (ok, _, stderr) = psql(
        &scratch,
        &[
            "-f",
            from_0010[0].join("down.sql").to_str().expect("a path"),
        ],
    );
    assert!(!ok && stderr.contains("destructive migration"), "{stderr}");

    must(
        &admin,
        &[
            "-c",
            &format!("DROP DATABASE IF EXISTS {SCRATCH} WITH (FORCE)"),
        ],
    );
}
