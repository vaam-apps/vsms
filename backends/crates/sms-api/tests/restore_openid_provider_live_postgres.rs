//! `0012_restore_openid_provider` (VSA-2, owner ruling R5): the five tables
//! `0009_drop_openid_provider` dropped (`oauth_clients`, `oauth_signing_keys`,
//! `client_assertions`, `user_credentials`, `sso_transactions`) come back, empty,
//! with the shape they had before `0009`.
//!
//! What this file holds the migration to:
//!
//! - **The shape is the original's.** A database that stopped at `0008` has the
//!   five tables as `0001_init`, `0002_bootstrap` and `0005_sso_federation`
//!   built them. Every column (order, type, nullability, default), constraint,
//!   index and trigger of that database is read from the catalogs and compared
//!   with a database migrated through `0012`, and with one that ran `0009` and
//!   was then given `0012` (the upgrade). A statement dropped from `0012`, or
//!   spelled differently, is a line that differs.
//! - **The foreign keys are plain.** `oauth_clients.app_client_id` and
//!   `user_credentials.user_id` refuse a parent's delete and cascade nothing.
//! - **The defaults, triggers and CHECKs work**, not just exist: the id and
//!   timestamp defaults, `touch_updated_at`, the client-registration CHECKs, the
//!   unique indexes the OP depends on for atomicity.
//! - **`down.sql` refuses while a row exists** and drops the tables when none
//!   does, in one transaction.
//! - **The runbook's export and import work.** The `\copy` statements of
//!   `docs/runbooks/deployment.adoc`, in the section `RUNBOOK_HEADING` names (how
//!   to keep the provider's rows across 0009 and 0012), are read out of the page
//!   and run.
//!
//! Plain `psql`, like the harness's own migration step and
//! `sealed_webhooks_migration_live_postgres.rs`: no generated client is involved,
//! because the point is the SQL and the "before" state predates the models.
//! Every test applies each migration the way `sms-migrate` does, in one
//! transaction (`psql -1`), on a scratch database of its own.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RUNBOOK: &str = include_str!("../../../../docs/runbooks/deployment.adoc");
const RUNBOOK_HEADING: &str = "=== Keeping the OpenID Provider's rows across 0009 and 0012";

/// The five tables, in the order `0009` drops them.
const TABLES: [&str; 5] = [
    "oauth_clients",
    "oauth_signing_keys",
    "client_assertions",
    "user_credentials",
    "sso_transactions",
];

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

/// Runs one `psql` invocation (in `dir` when given, feeding `stdin` when given)
/// and returns `(success, stdout, stderr)`.
fn psql_in(
    url: &str,
    args: &[&str],
    dir: Option<&Path>,
    stdin: Option<&str>,
) -> (bool, String, String) {
    let mut command = Command::new("psql");
    command
        .arg(url)
        .args(["-v", "ON_ERROR_STOP=1", "-q", "-At"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .expect("spawning `psql` — is the postgresql-client package installed?");
    {
        use std::io::Write;
        let mut pipe = child.stdin.take().expect("a piped stdin");
        if let Some(stdin) = stdin {
            pipe.write_all(stdin.as_bytes()).expect("writing to psql");
        }
    }
    let output = child.wait_with_output().expect("waiting for psql");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    )
}

fn psql(url: &str, args: &[&str]) -> (bool, String, String) {
    psql_in(url, args, None, None)
}

fn must(url: &str, args: &[&str]) -> String {
    let (ok, stdout, stderr) = psql(url, args);
    assert!(ok, "psql {args:?} failed: {stderr}");
    stdout
}

/// A statement that must be refused: returns what `psql` said on stderr.
fn refused(url: &str, sql: &str) -> String {
    let (ok, _, stderr) = psql(url, &["-c", sql]);
    assert!(!ok, "{sql} was accepted, but must be refused");
    stderr
}

/// Applies one migration file the way `sms-migrate` does: in one transaction.
fn apply(url: &str, file: &Path) {
    let file = file.to_str().expect("a UTF-8 path");
    must(url, &["-1", "-f", file]);
}

/// The migration directories whose four-digit number is at most `through`
/// (`"0008"`), in apply order.
fn migrations_through(through: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(migrations_root())
        .expect("reading the migrations")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name[..4] <= *through)
        })
        .collect();
    dirs.sort();
    dirs
}

fn migration(number: &str) -> PathBuf {
    let found: Vec<PathBuf> = migrations_through(number)
        .into_iter()
        .filter(|dir| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(number))
        })
        .collect();
    assert_eq!(found.len(), 1, "exactly one migration is {number}");
    found.into_iter().next().expect("one")
}

/// A scratch database migrated through `through`, dropped when the test ends.
struct Scratch {
    admin: String,
    name: String,
    url: String,
}

impl Scratch {
    async fn through(name: &str, through: &str) -> Self {
        let base = sms_test_support::database_url().await;
        let admin = with_database(&base, "postgres");
        let name = format!("vsms_mig0012_{name}");
        let url = with_database(&base, &name);
        drop_database(&admin, &name);
        must(&admin, &["-c", &format!("CREATE DATABASE {name}")]);
        let scratch = Self { admin, name, url };
        scratch.apply_through(through);
        scratch
    }

    fn apply_through(&self, through: &str) {
        for dir in migrations_through(through) {
            for file in ["up.pre.sql", "up.sql"] {
                let path = dir.join(file);
                if path.exists() {
                    apply(&self.url, &path);
                }
            }
        }
    }

    /// Applies one migration's `up.sql`.
    fn apply_up(&self, number: &str) {
        apply(&self.url, &migration(number).join("up.sql"));
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop_database(&self.admin, &self.name);
    }
}

fn drop_database(admin: &str, name: &str) {
    must(
        admin,
        &[
            "-c",
            &format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        ],
    );
}

fn table_list() -> String {
    TABLES
        .iter()
        .map(|table| format!("'{table}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Everything the catalogs say about the five tables, one line each: the tables
/// themselves, every column (position, type, nullability, default), every
/// constraint (primary key, unique, check, foreign key, with its definition),
/// every index and every non-internal trigger. Also the table's privileges
/// (`relacl`, null until a `GRANT`/`REVOKE`), whether row-level security is on
/// (`relrowsecurity`) and whether each trigger is enabled (`tgenabled`), so an
/// extra `GRANT`, `ENABLE ROW LEVEL SECURITY` or `DISABLE TRIGGER` fails it. Two databases with equal
/// fingerprints have the same tables.
fn fingerprint(url: &str) -> String {
    let tables = table_list();
    must(
        url,
        &[
            "-c",
            &format!(
                "SELECT line FROM (
                   SELECT format('rel|%s|%s|acl=%s|rls=%s', c.relname, c.relkind,
                                 coalesce(c.relacl::text, ''), c.relrowsecurity) AS line
                     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                    WHERE n.nspname = 'public' AND c.relname IN ({tables})
                   UNION ALL
                   SELECT format('col|%s|%s|%s|%s|%s|%s', c.relname, a.attnum, a.attname,
                                 format_type(a.atttypid, a.atttypmod), a.attnotnull,
                                 coalesce(pg_get_expr(d.adbin, d.adrelid), ''))
                     FROM pg_class c
                     JOIN pg_namespace n ON n.oid = c.relnamespace
                     JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
                     LEFT JOIN pg_attrdef d ON d.adrelid = c.oid AND d.adnum = a.attnum
                    WHERE n.nspname = 'public' AND c.relname IN ({tables})
                   UNION ALL
                   SELECT format('con|%s|%s|%s|%s', c.relname, k.conname, k.contype,
                                 pg_get_constraintdef(k.oid))
                     FROM pg_constraint k
                     JOIN pg_class c ON c.oid = k.conrelid
                     JOIN pg_namespace n ON n.oid = c.relnamespace
                    WHERE n.nspname = 'public' AND c.relname IN ({tables})
                   UNION ALL
                   SELECT format('idx|%s|%s|%s', tablename, indexname, indexdef)
                     FROM pg_indexes
                    WHERE schemaname = 'public' AND tablename IN ({tables})
                   UNION ALL
                   SELECT format('trg|%s|%s|enabled=%s|%s', c.relname, t.tgname, t.tgenabled,
                                 pg_get_triggerdef(t.oid))
                     FROM pg_trigger t
                     JOIN pg_class c ON c.oid = t.tgrelid
                     JOIN pg_namespace n ON n.oid = c.relnamespace
                    WHERE n.nspname = 'public' AND c.relname IN ({tables}) AND NOT t.tgisinternal
                 ) shape ORDER BY line"
            ),
        ],
    )
}

/// `actual` equals `expected`, line for line; on a difference, panics naming only
/// the lines one side has and the other lacks (a fingerprint is a few dozen lines
/// and an `assert_eq!` on it prints them all twice).
fn assert_same_shape(actual: &str, expected: &str, what: &str) {
    let only_in = |a: &str, b: &str| -> Vec<String> {
        a.lines()
            .filter(|line| !b.lines().any(|other| other == *line))
            .map(str::to_owned)
            .collect()
    };
    let missing = only_in(expected, actual);
    let extra = only_in(actual, expected);
    assert!(
        missing.is_empty() && extra.is_empty() && actual == expected,
        "{what}\n  missing (as before 0009, not here): {missing:#?}\n  extra (here, not before 0009): {extra:#?}"
    );
}

fn existing_tables(url: &str) -> Vec<String> {
    let tables = table_list();
    must(
        url,
        &[
            "-c",
            &format!(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = 'public' AND table_name IN ({tables}) ORDER BY table_name"
            ),
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

fn row_count(url: &str, table: &str) -> i64 {
    must(url, &["-c", &format!("SELECT count(*) FROM {table}")])
        .parse()
        .expect("a count")
}

/// What a database that stopped at `0008` has: the five tables as they were.
/// Computed on its own scratch database each time it is wanted, so no test
/// leans on another's.
async fn the_shape_before_0009(name: &str) -> String {
    let before = Scratch::through(name, "0008").await;
    let shape = fingerprint(&before.url);
    assert_eq!(
        existing_tables(&before.url).len(),
        5,
        "0008 still has all five tables"
    );
    shape
}

/// Sanity on the fingerprint itself, so an equal pair of empty strings cannot
/// pass: it names every table, the hand-written CHECKs, the partial index and
/// the foreign keys.
fn assert_fingerprint_is_not_vacuous(shape: &str) {
    for needle in [
        "rel|oauth_clients|r|acl=|rls=f",
        "rel|sso_transactions|r|acl=|rls=f",
        "col|oauth_clients|6|token_endpoint_auth_method|text|t|",
        "col|oauth_clients|8|grant_types|text|t|' '::text",
        "col|user_credentials|3|id|text|t|cs_cuid()",
        "oauth_clients_auth_method_jwks_check|c|",
        "oauth_clients_public_requires_pkce_check|c|",
        "oauth_clients_token_endpoint_auth_method_enum_check|c|",
        "oauth_clients_app_client_id_fkey|f|FOREIGN KEY (app_client_id) REFERENCES app_clients(id)",
        "user_credentials_user_id_fkey|f|FOREIGN KEY (user_id) REFERENCES users(id)",
        "idx|oauth_signing_keys|oauth_signing_keys_active_idx|",
        "WHERE active",
        "idx|client_assertions|client_assertions_expiry_idx|",
        "idx|sso_transactions|sso_transactions_expiry_idx|",
        "idx|sso_transactions|sso_transactions_state_hash_key|CREATE UNIQUE INDEX",
        "trg|oauth_clients|oauth_clients_touch|enabled=O|",
        "trg|user_credentials|user_credentials_touch|enabled=O|",
    ] {
        assert!(shape.contains(needle), "the fingerprint lacks `{needle}`");
    }
    assert!(
        !shape.contains("trg|sso_transactions"),
        "sso_transactions has no updated_at and so no touch trigger"
    );
}

/// A fresh database, migrated through `0012` in order, ends with the five
/// tables exactly as a database that stopped at `0008` has them.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_fresh_database_through_0012_has_the_five_tables_as_before_0009() {
    let reference = the_shape_before_0009("fresh_ref").await;
    assert_fingerprint_is_not_vacuous(&reference);

    let fresh = Scratch::through("fresh", "0012").await;
    assert_eq!(existing_tables(&fresh.url), {
        let mut sorted = TABLES.map(str::to_owned).to_vec();
        sorted.sort();
        sorted
    });
    assert_same_shape(
        &fingerprint(&fresh.url),
        &reference,
        "a fresh database through 0012 differs from one that stopped at 0008",
    );
    for table in TABLES {
        assert_eq!(row_count(&fresh.url, table), 0, "{table} starts empty");
    }
}

/// A database already through `0011` (so through `0009`, which dropped the
/// tables, and with rows in the tables `0009` kept) ends with the same tables
/// when `0012` runs, and `0012` touches nothing else.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_database_through_0011_gets_them_from_0012() {
    let reference = the_shape_before_0009("upgrade_ref").await;

    let upgraded = Scratch::through("upgrade", "0011").await;
    assert_eq!(
        existing_tables(&upgraded.url),
        Vec::<String>::new(),
        "0009 dropped them, and 0010 and 0011 do not bring them back"
    );
    // `federated_links` survived 0009 and must survive 0012 untouched.
    must(
        &upgraded.url,
        &[
            "-c",
            "INSERT INTO roles (key, label, permissions, version) \
             VALUES ('r12', 'r12', ' ', 1) ON CONFLICT DO NOTHING; \
             INSERT INTO users (id, subject, email, display_name, role_key, version) \
             VALUES ('usr1', 'usr1', 'u@e.test', 'U', 'r12', 1); \
             INSERT INTO federated_links (issuer, subject, user_id) \
             VALUES ('vaam', 'v-usr1', 'usr1')",
        ],
    );
    let tables_before = must(
        &upgraded.url,
        &[
            "-c",
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' ORDER BY table_name",
        ],
    );

    upgraded.apply_up("0012");

    assert_same_shape(
        &fingerprint(&upgraded.url),
        &reference,
        "a database through 0011 plus 0012 differs from one that stopped at 0008",
    );
    for table in TABLES {
        assert_eq!(row_count(&upgraded.url, table), 0, "{table} starts empty");
    }
    let tables_after = must(
        &upgraded.url,
        &[
            "-c",
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' ORDER BY table_name",
        ],
    );
    let added: Vec<&str> = tables_after
        .lines()
        .filter(|table| !tables_before.lines().any(|before| before == *table))
        .collect();
    let mut expected = TABLES.to_vec();
    expected.sort_unstable();
    assert_eq!(
        added, expected,
        "0012 adds the five tables and nothing else"
    );
    assert_eq!(
        must(
            &upgraded.url,
            &["-c", "SELECT issuer, subject, user_id FROM federated_links"]
        ),
        "vaam|v-usr1|usr1",
        "federated_links is not touched"
    );
}

/// The migration does not run twice: a second `up` over the tables it made is
/// refused, loudly, instead of quietly adopting whatever is there.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_second_run_is_refused_rather_than_adopting_the_tables() {
    let db = Scratch::through("twice", "0012").await;
    let up = migration("0012").join("up.sql");
    let (ok, _, stderr) = psql(&db.url, &["-1", "-f", up.to_str().expect("a UTF-8 path")]);
    assert!(!ok, "0012 over a database that has the tables must fail");
    assert!(stderr.contains("already exists"), "{stderr}");
}

fn seed_parents(url: &str) {
    must(
        url,
        &[
            "-c",
            "INSERT INTO apps (id, name, slug, monthly_quota, ip_allowlist, \
                               transliterate_to_gsm7, version) \
             VALUES ('app1', 'an app', 'an-app', 1000, ' ', false, 1); \
             INSERT INTO app_clients (id, app_id, client_id, label, scopes, version) \
             VALUES ('apc1', 'app1', 'cli-1', 'client', ' ', 1); \
             INSERT INTO users (id, subject, email, display_name, role_key, version) \
             VALUES ('usr1', 'usr1', 'u@e.test', 'U', 'owner', 1)",
        ],
    );
}

const JWKS: &str = r#"{"keys":[{"kty":"OKP","crv":"Ed25519","x":"AAAA"}]}"#;

fn insert_client(client_id: &str, parent: &str, method: &str, jwks: &str, pkce: bool) -> String {
    let jwks = if jwks == "NULL" {
        "NULL".to_owned()
    } else {
        format!("'{jwks}'")
    };
    format!(
        "INSERT INTO oauth_clients (client_id, app_client_id, token_endpoint_auth_method, jwks, require_pkce) \
         VALUES ('{client_id}', {parent}, '{method}', {jwks}, {pkce})"
    )
}

/// The foreign keys are the generator's plain `NO ACTION` ones: a parent that
/// one of these rows names cannot be deleted, nothing cascades either way, and a
/// row cannot name a parent that is not there.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_foreign_keys_refuse_a_parents_delete_and_cascade_nothing() {
    let db = Scratch::through("fk", "0012").await;
    seed_parents(&db.url);
    must(
        &db.url,
        &[
            "-c",
            &insert_client("client-001", "'apc1'", "private_key_jwt", JWKS, false),
        ],
    );
    must(
        &db.url,
        &[
            "-c",
            "INSERT INTO user_credentials (user_id, password_hash) \
             VALUES ('usr1', '$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA')",
        ],
    );

    let error = refused(&db.url, "DELETE FROM app_clients WHERE id = 'apc1'");
    assert!(
        error.contains("oauth_clients_app_client_id_fkey"),
        "{error}"
    );
    let error = refused(&db.url, "DELETE FROM users WHERE id = 'usr1'");
    assert!(error.contains("user_credentials_user_id_fkey"), "{error}");
    // Refused, not cascaded: parent and child are both still there.
    assert_eq!(row_count(&db.url, "app_clients"), 1);
    assert_eq!(row_count(&db.url, "oauth_clients"), 1);
    assert_eq!(row_count(&db.url, "users"), 1);
    assert_eq!(row_count(&db.url, "user_credentials"), 1);

    // A row cannot name a parent that is not there.
    let error = refused(
        &db.url,
        &insert_client("client-002", "'nope'", "private_key_jwt", JWKS, false),
    );
    assert!(
        error.contains("oauth_clients_app_client_id_fkey"),
        "{error}"
    );
    let error = refused(
        &db.url,
        "INSERT INTO user_credentials (user_id, password_hash) VALUES ('nobody', 'h')",
    );
    assert!(error.contains("user_credentials_user_id_fkey"), "{error}");
    // `app_client_id` is optional: a public (PKCE) console client names none.
    must(
        &db.url,
        &[
            "-c",
            &insert_client("console-01", "NULL", "none", "NULL", true),
        ],
    );

    // Deleting the child leaves the parent, and then the parent can go.
    must(&db.url, &["-c", "DELETE FROM oauth_clients"]);
    assert_eq!(
        row_count(&db.url, "app_clients"),
        1,
        "the child's delete takes no parent"
    );
    must(
        &db.url,
        &["-c", "DELETE FROM app_clients WHERE id = 'apc1'"],
    );
    must(&db.url, &["-c", "DELETE FROM user_credentials"]);
    must(&db.url, &["-c", "DELETE FROM users WHERE id = 'usr1'"]);
}

/// The hand-written half of the migration does its work: ids and timestamps
/// default, `touch_updated_at` fires, the client-registration CHECKs refuse what
/// they were written to refuse, and the unique indexes the OP's atomic
/// "record if unseen" depends on refuse a second row.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_defaults_triggers_checks_and_unique_indexes_work() {
    let db = Scratch::through("rules", "0012").await;
    seed_parents(&db.url);
    ids_and_timestamps_default_and_lists_default_to_a_space(&db);
    touch_updated_at_fires_and_an_sso_row_takes_its_defaults(&db);
    the_registration_checks_refuse_what_they_were_written_to_refuse(&db);
    the_unique_indexes_refuse_a_second_row(&db);
}

/// A row that names no id and no timestamp gets them, and the list columns default to a space.
fn ids_and_timestamps_default_and_lists_default_to_a_space(db: &Scratch) {
    must(
        &db.url,
        &[
            "-c",
            &insert_client("client-001", "'apc1'", "private_key_jwt", JWKS, false),
        ],
    );
    assert_eq!(
        must(
            &db.url,
            &[
                "-c",
                "SELECT id ~ '^c[0-9a-f]{22}$', grant_types = ' ', scopes = ' ', \
                        redirect_uris = ' ', active, created_at IS NOT NULL, updated_at IS NOT NULL \
                   FROM oauth_clients"
            ]
        ),
        "t|t|t|t|t|t|t",
        "cs_cuid() and the list and timestamp defaults"
    );
}

/// An update moves `updated_at` past `created_at` on the four tables that have the
/// column, and an `sso_transactions` row (which has none) takes its id and
/// `created_at` defaults.
fn touch_updated_at_fires_and_an_sso_row_takes_its_defaults(db: &Scratch) {
    // touch_updated_at: an update moves updated_at past created_at.
    must(&db.url, &["-c", "UPDATE oauth_clients SET active = false"]);
    assert_eq!(
        must(
            &db.url,
            &["-c", "SELECT updated_at > created_at FROM oauth_clients"]
        ),
        "t",
        "oauth_clients_touch"
    );
    must(
        &db.url,
        &[
            "-c",
            "INSERT INTO oauth_signing_keys (private_key_pem) VALUES ('-----BEGIN PRIVATE KEY-----')",
        ],
    );
    must(
        &db.url,
        &["-c", "UPDATE oauth_signing_keys SET active = false"],
    );
    assert_eq!(
        must(
            &db.url,
            &[
                "-c",
                "SELECT updated_at > created_at, NOT active FROM oauth_signing_keys"
            ]
        ),
        "t|t",
        "oauth_signing_keys_touch (and `active` defaulted to true before)"
    );
    must(
        &db.url,
        &[
            "-c",
            "INSERT INTO client_assertions (jti, expires_at) VALUES ('jti-1', now() + interval '1 hour')",
        ],
    );
    must(
        &db.url,
        &["-c", "UPDATE client_assertions SET expires_at = now()"],
    );
    assert_eq!(
        must(
            &db.url,
            &[
                "-c",
                "SELECT updated_at > created_at FROM client_assertions"
            ]
        ),
        "t",
        "client_assertions_touch"
    );
    must(
        &db.url,
        &[
            "-c",
            "INSERT INTO user_credentials (user_id, password_hash) VALUES ('usr1', 'h')",
        ],
    );
    must(
        &db.url,
        &["-c", "UPDATE user_credentials SET password_hash = 'h2'"],
    );
    assert_eq!(
        must(
            &db.url,
            &["-c", "SELECT updated_at > created_at FROM user_credentials"]
        ),
        "t",
        "user_credentials_touch"
    );
    must(
        &db.url,
        &[
            "-c",
            &format!(
                "INSERT INTO sso_transactions (state_hash, nonce, code_verifier, console_request, is_retry, expires_at) \
                 VALUES ('{}', 'n', 'v', '{{}}', false, now() + interval '5 minutes')",
                "a".repeat(64)
            ),
        ],
    );
    assert_eq!(
        must(
            &db.url,
            &[
                "-c",
                "SELECT id ~ '^c[0-9a-f]{22}$', created_at IS NOT NULL FROM sso_transactions"
            ]
        ),
        "t|t"
    );
}

/// The client-registration CHECKs refuse each wrong shape, and nothing refused is stored.
fn the_registration_checks_refuse_what_they_were_written_to_refuse(db: &Scratch) {
    // The client-registration CHECKs.
    for (label, sql, constraint) in [
        (
            "private_key_jwt with an empty key set",
            insert_client(
                "client-e1",
                "NULL",
                "private_key_jwt",
                r#"{"keys":[]}"#,
                false,
            ),
            "oauth_clients_auth_method_jwks_check",
        ),
        (
            "private_key_jwt with no key at all",
            insert_client("client-e2", "NULL", "private_key_jwt", "NULL", false),
            "oauth_clients_auth_method_jwks_check",
        ),
        (
            "a public client that holds a key",
            insert_client("client-e3", "NULL", "none", JWKS, true),
            "oauth_clients_auth_method_jwks_check",
        ),
        (
            "a public client without PKCE",
            insert_client("client-e4", "NULL", "none", "NULL", false),
            "oauth_clients_public_requires_pkce_check",
        ),
        (
            // Postgres checks CHECKs in name order, and the jwks one admits only
            // the two methods the enum names, so it answers first; the enum's own
            // CHECK is in the fingerprint test and cannot be reached before it.
            "an auth method the enum does not name",
            insert_client("client-e5", "NULL", "client_secret_basic", JWKS, true),
            "oauth_clients_auth_method_jwks_check",
        ),
    ] {
        let error = refused(&db.url, &sql);
        assert!(error.contains(constraint), "{label}: {error}");
    }
    assert_eq!(
        row_count(&db.url, "oauth_clients"),
        1,
        "nothing refused was stored"
    );
}

/// A second row with the same `client_id`, `jti`, user or SSO state is refused.
fn the_unique_indexes_refuse_a_second_row(db: &Scratch) {
    // The unique indexes.
    for (label, sql, index) in [
        (
            "client_id",
            insert_client("client-001", "NULL", "private_key_jwt", JWKS, false),
            "oauth_clients_client_id_key",
        ),
        (
            "jti: a replayed assertion",
            "INSERT INTO client_assertions (jti, expires_at) VALUES ('jti-1', now())".to_owned(),
            "client_assertions_jti_key",
        ),
        (
            "one credential per user",
            "INSERT INTO user_credentials (user_id, password_hash) VALUES ('usr1', 'other')"
                .to_owned(),
            "user_credentials_user_id_key",
        ),
        (
            "a replayed SSO state",
            format!(
                "INSERT INTO sso_transactions (state_hash, nonce, code_verifier, console_request, is_retry, expires_at) \
                 VALUES ('{}', 'n', 'v', '{{}}', false, now())",
                "a".repeat(64)
            ),
            "sso_transactions_state_hash_key",
        ),
    ] {
        let error = refused(&db.url, &sql);
        assert!(error.contains(index), "{label}: {error}");
    }
}

/// `down.sql` counts the rows first and refuses while any table holds one,
/// naming the counts and leaving everything as it was; with the tables empty it
/// drops exactly the five; and `up.sql` then brings the same shape back.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn down_refuses_while_a_row_exists_and_drops_the_five_when_none_does() {
    let reference = the_shape_before_0009("down_ref").await;
    let db = Scratch::through("down", "0012").await;
    let down = migration("0012").join("down.sql");
    let down = down.to_str().expect("a UTF-8 path");

    must(
        &db.url,
        &[
            "-c",
            "INSERT INTO oauth_signing_keys (private_key_pem) VALUES ('pem')",
        ],
    );
    let (ok, _, stderr) = psql(&db.url, &["-f", down]);
    assert!(!ok, "a rollback over a signing key must be refused");
    assert!(
        stderr.contains("cannot roll back 0012")
            && stderr.contains("oauth_signing_keys=1")
            && stderr.contains("oauth_clients=0"),
        "{stderr}"
    );
    assert_eq!(
        existing_tables(&db.url).len(),
        5,
        "the refusal changed nothing"
    );
    assert_same_shape(
        &fingerprint(&db.url),
        &reference,
        "after the refused rollback the tables differ from 0008",
    );
    assert_eq!(row_count(&db.url, "oauth_signing_keys"), 1);

    must(&db.url, &["-c", "DELETE FROM oauth_signing_keys"]);
    must(&db.url, &["-f", down]);
    assert_eq!(
        existing_tables(&db.url),
        Vec::<String>::new(),
        "down drops the five"
    );
    // ... and nothing else: the tables 0009 kept are still there.
    assert_eq!(row_count(&db.url, "federated_links"), 0);
    assert_eq!(row_count(&db.url, "app_clients"), 0);

    // Forward again: the same shape.
    db.apply_up("0012");
    assert_same_shape(
        &fingerprint(&db.url),
        &reference,
        "after down then up the tables differ from 0008",
    );
}

/// Spent client assertions and SSO sign-ins are the ones the rollback counts
/// too: a table that holds only ephemeral rows still refuses it.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn down_counts_every_one_of_the_five_tables() {
    let db = Scratch::through("downall", "0012").await;
    seed_parents(&db.url);
    let down = migration("0012").join("down.sql");
    let down = down.to_str().expect("a UTF-8 path");
    let rows = [
        (
            "oauth_clients",
            insert_client("client-001", "'apc1'", "private_key_jwt", JWKS, false),
        ),
        (
            "oauth_signing_keys",
            "INSERT INTO oauth_signing_keys (private_key_pem) VALUES ('pem')".to_owned(),
        ),
        (
            "client_assertions",
            "INSERT INTO client_assertions (jti, expires_at) VALUES ('j', now())".to_owned(),
        ),
        (
            "user_credentials",
            "INSERT INTO user_credentials (user_id, password_hash) VALUES ('usr1', 'h')".to_owned(),
        ),
        (
            "sso_transactions",
            format!(
                "INSERT INTO sso_transactions (state_hash, nonce, code_verifier, console_request, is_retry, expires_at) \
                 VALUES ('{}', 'n', 'v', '{{}}', false, now())",
                "b".repeat(64)
            ),
        ),
    ];
    for (table, insert) in &rows {
        must(&db.url, &["-c", insert]);
        let (ok, _, stderr) = psql(&db.url, &["-f", down]);
        assert!(!ok, "a rollback over a {table} row must be refused");
        assert!(stderr.contains(&format!("{table}=1")), "{table}: {stderr}");
        must(&db.url, &["-c", &format!("DELETE FROM {table}")]);
    }
    must(&db.url, &["-f", down]);
    assert_eq!(existing_tables(&db.url).len(), 0);
}

/// The `\copy` statements of the runbook's section, in page order: the exports
/// (`TO`) and the imports (`FROM`), each as the page writes them.
fn runbook_copies() -> (String, String) {
    let start = RUNBOOK
        .find(RUNBOOK_HEADING)
        .expect("docs/runbooks/deployment.adoc no longer has the section this test runs");
    let section = &RUNBOOK[start + RUNBOOK_HEADING.len()..];
    let end = section
        .find("\n== ")
        .into_iter()
        .chain(section.find("\n=== "))
        .min()
        .unwrap_or(section.len());
    let mut exports = String::new();
    let mut imports = String::new();
    for line in section[..end].lines().filter(|l| l.starts_with("\\copy ")) {
        let target = if line.contains(" TO '") {
            &mut exports
        } else {
            assert!(line.contains(" FROM '"), "{line}");
            &mut imports
        };
        target.push_str(line);
        target.push('\n');
    }
    for table in ["oauth_signing_keys", "oauth_clients", "user_credentials"] {
        assert!(
            exports.contains(&format!("\\copy {table} (")),
            "export of {table}"
        );
        assert!(
            imports.contains(&format!("\\copy {table} (")),
            "import of {table}"
        );
    }
    (exports, imports)
}

const ROWS: &str = "SELECT md5(string_agg(t::text, E'\\n' ORDER BY t::text)) FROM (\
                    SELECT * FROM TABLE ORDER BY id) t";

fn digest(url: &str, table: &str) -> String {
    must(url, &["-c", &ROWS.replace("TABLE", table)])
}

/// The runbook's way to keep the first three tables across `0009` and `0012`:
/// rows written under the old shape, exported with the page's own statements
/// before the upgrade, come back byte for byte after it. The same upgrade
/// without the import ends with empty tables (the loss the page names).
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_provider_export_taken_before_0009_is_restored_after_0012() {
    let (exports, imports) = runbook_copies();
    let db = Scratch::through("export", "0008").await;
    seed_parents(&db.url);
    must(
        &db.url,
        &[
            "-c",
            "INSERT INTO oauth_signing_keys (id, private_key_pem, active, expires_at) \
             VALUES ('ck1', E'-----BEGIN PRIVATE KEY-----\\nline, \"two\"\\n-----END PRIVATE KEY-----', true, NULL), \
                    ('ck2', 'older', false, '2026-12-31T00:00:00Z')",
        ],
    );
    must(
        &db.url,
        &[
            "-c",
            "INSERT INTO oauth_clients (id, client_id, app_client_id, token_endpoint_auth_method, jwks, \
                                       grant_types, scopes, redirect_uris, require_pkce, active) \
             VALUES ('oc1', 'client-001', 'apc1', 'private_key_jwt', \
                     '{\"keys\":[{\"kty\":\"OKP\",\"x\":\"AAAA\"}]}', \
                     ' client_credentials ', ' sms:send sms:read ', ' ', false, true), \
                    ('oc2', 'console-01', NULL, 'none', NULL, \
                     ' authorization_code ', ' ', ' https://c.example.test/cb ', true, false)",
        ],
    );
    must(
        &db.url,
        &[
            "-c",
            "INSERT INTO user_credentials (id, user_id, password_hash) \
             VALUES ('uc1', 'usr1', '$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA')",
        ],
    );
    let tables = ["oauth_signing_keys", "oauth_clients", "user_credentials"];
    let before: Vec<String> = tables.iter().map(|t| digest(&db.url, t)).collect();

    // The export: the page's statements, run from a directory of their own.
    let dir = std::env::temp_dir().join(format!("vsms-op-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("export directory");
    let (ok, _, stderr) = psql_in(&db.url, &[], Some(&dir), Some(&exports));
    assert!(ok, "the runbook's export failed: {stderr}");

    // The upgrade: 0009 drops the tables, 0010 to 0012 follow.
    for number in ["0009", "0010", "0011", "0012"] {
        db.apply_up(number);
    }
    for table in TABLES {
        assert_eq!(
            row_count(&db.url, table),
            0,
            "{table} is empty after the upgrade"
        );
    }

    // The import: one transaction, as the page runs it (`-1`).
    let (ok, _, stderr) = psql_in(&db.url, &["-1"], Some(&dir), Some(&imports));
    assert!(ok, "the runbook's import failed: {stderr}");
    let after: Vec<String> = tables.iter().map(|t| digest(&db.url, t)).collect();
    assert_eq!(before, after, "every exported row came back as it was");
    assert_eq!(row_count(&db.url, "oauth_signing_keys"), 2);
    assert_eq!(row_count(&db.url, "oauth_clients"), 2);
    assert_eq!(row_count(&db.url, "user_credentials"), 1);
    // Ephemeral tables are not exported, and stay empty.
    assert_eq!(row_count(&db.url, "client_assertions"), 0);
    assert_eq!(row_count(&db.url, "sso_transactions"), 0);

    // The import is all or nothing: a user deleted since the export refuses it,
    // and the tables stay empty rather than half full.
    must(
        &db.url,
        &[
            "-c",
            "DELETE FROM user_credentials; DELETE FROM oauth_clients; DELETE FROM oauth_signing_keys; DELETE FROM users WHERE id = 'usr1'",
        ],
    );
    let (ok, _, stderr) = psql_in(&db.url, &["-1"], Some(&dir), Some(&imports));
    assert!(!ok, "an import naming a deleted user must be refused");
    assert!(stderr.contains("user_credentials_user_id_fkey"), "{stderr}");
    for table in tables {
        assert_eq!(
            row_count(&db.url, table),
            0,
            "{table}: the refused import left nothing"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The runbook's cleanup unblocks what an import blocks: with a row of
/// `oauth_clients` and one of `user_credentials` naming them, the `AppClient` and
/// the User cannot be deleted (a plain `NO ACTION` foreign key); after the page's
/// own `DELETE` statements, with its placeholders filled in, both can.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_runbooks_cleanup_unblocks_a_hard_delete() {
    let start = RUNBOOK
        .find(RUNBOOK_HEADING)
        .expect("docs/runbooks/deployment.adoc no longer has the section this test runs");
    let section = &RUNBOOK[start + RUNBOOK_HEADING.len()..];
    let end = section
        .find("\n== ")
        .into_iter()
        .chain(section.find("\n=== "))
        .min()
        .unwrap_or(section.len());
    let cleanup: Vec<String> = section[..end]
        .lines()
        .filter(|l| l.starts_with("DELETE FROM "))
        .map(|l| {
            l.replace("<app client id>", "apc1")
                .replace("<user id>", "usr1")
        })
        .collect();
    assert_eq!(cleanup.len(), 2, "the page gives two DELETE statements");

    let db = Scratch::through("cleanup", "0012").await;
    seed_parents(&db.url);
    must(
        &db.url,
        &[
            "-c",
            &insert_client("client-001", "'apc1'", "private_key_jwt", JWKS, false),
        ],
    );
    must(
        &db.url,
        &[
            "-c",
            "INSERT INTO user_credentials (user_id, password_hash) VALUES ('usr1', 'h')",
        ],
    );
    let error = refused(&db.url, "DELETE FROM app_clients WHERE id = 'apc1'");
    assert!(
        error.contains("oauth_clients_app_client_id_fkey"),
        "{error}"
    );
    let error = refused(&db.url, "DELETE FROM users WHERE id = 'usr1'");
    assert!(error.contains("user_credentials_user_id_fkey"), "{error}");

    for statement in &cleanup {
        must(&db.url, &["-c", statement]);
    }
    assert_eq!(row_count(&db.url, "oauth_clients"), 0);
    assert_eq!(row_count(&db.url, "user_credentials"), 0);
    must(
        &db.url,
        &["-c", "DELETE FROM app_clients WHERE id = 'apc1'"],
    );
    must(&db.url, &["-c", "DELETE FROM users WHERE id = 'usr1'"]);
}
