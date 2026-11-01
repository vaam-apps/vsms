//! `0015_delivery_lookups`: the `DeliveryLookup` table, on a database that already
//! ran `0001`..`0014`.
//!
//! What this file holds the migration to:
//!
//! - **One new table and nothing else.** The table list gains `delivery_lookups`
//!   and loses nothing, and the `roles` table, the one every earlier migration of
//!   this series touched, is byte-identical (`updated_at` included): `delivery:read`
//!   was seeded long ago and this migration grants it to no one.
//! - **The columns the model has**, with the types the generator writes, and the
//!   defaults the emitter cannot (`cs_cuid()` for `id`, `now()` for `created_at`).
//! - **The CHECKs refuse what the model's validators would**: a key kind outside the
//!   enum, an empty or oversized actor or key, a negative count.
//! - **The budget's index** exists, on the caller and the time, newest first.
//! - **`down.sql`** drops the table and its index and nothing else.
//!
//! Plain `psql`, like `optout_read_scope_migration_live_postgres.rs`, which this file
//! is modelled on. Each migration is applied the way `sms-migrate` does, in one
//! transaction (`psql -1`), on a scratch database of its own.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The directory name of the migration this file is about.
const THIS: &str = "0015_delivery_lookups";

/// One test at a time: they share the scratch databases' prefix and one server.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn migrations_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../migrations/postgres")
        .canonicalize()
        .expect("backends/migrations/postgres")
}

fn with_database(url: &str, database: &str) -> String {
    let (head, _) = url.rsplit_once('/').expect("a postgres URL has a path");
    format!("{head}/{database}")
}

fn psql(url: &str, args: &[&str]) -> (bool, String, String) {
    let output = Command::new("psql")
        .arg(url)
        .args(["-v", "ON_ERROR_STOP=1", "-q", "-At"])
        .args(args)
        .output()
        .expect("spawning `psql` - is the postgresql-client package installed?");
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

fn sql(url: &str, statement: &str) -> String {
    must(url, &["-c", statement])
}

/// The error text of a statement that must be refused.
fn refused(url: &str, statement: &str) -> String {
    let (ok, stdout, stderr) = psql(url, &["-c", statement]);
    assert!(!ok, "{statement} was accepted: {stdout}");
    stderr
}

fn apply(url: &str, file: &Path) {
    must(url, &["-1", "-f", file.to_str().expect("a UTF-8 path")]);
}

fn migration_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(migrations_root())
        .expect("reading the migrations")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn this_migration() -> PathBuf {
    migration_dirs()
        .into_iter()
        .find(|dir| dir.ends_with(THIS))
        .expect("0015_delivery_lookups exists")
}

/// A scratch database migrated through `0014`, dropped when the test ends.
struct Scratch {
    admin: String,
    name: String,
    url: String,
}

impl Scratch {
    async fn before_0015(name: &str) -> Self {
        let base = sms_test_support::database_url().await;
        let admin = with_database(&base, "postgres");
        let name = format!("vsms_mig0015_{name}");
        let url = with_database(&base, &name);
        drop_database(&admin, &name);
        must(&admin, &["-c", &format!("CREATE DATABASE {name}")]);
        let scratch = Self { admin, name, url };
        for dir in migration_dirs()
            .into_iter()
            .take_while(|dir| !dir.ends_with(THIS))
        {
            for file in ["up.pre.sql", "up.sql"] {
                let path = dir.join(file);
                if path.exists() {
                    apply(&scratch.url, &path);
                }
            }
        }
        scratch
    }

    fn up(&self) {
        apply(&self.url, &this_migration().join("up.sql"));
    }

    fn down(&self) {
        apply(&self.url, &this_migration().join("down.sql"));
    }

    fn tables(&self) -> Vec<String> {
        sql(
            &self.url,
            "SELECT table_name FROM information_schema.tables \
              WHERE table_schema = 'public' ORDER BY table_name",
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }

    fn roles(&self) -> String {
        sql(
            &self.url,
            "SELECT key, builtin, '[' || permissions || ']', version, updated_at \
               FROM roles ORDER BY key",
        )
    }
}

fn insert(columns: &str, values: &str) -> String {
    format!("INSERT INTO delivery_lookups ({columns}) VALUES ({values})")
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

const COLUMNS: &str = "actor_sub, actor_kind, actor_role, key_kind, key_value, result_count";

#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn the_migration_adds_one_table_and_moves_no_role() {
    let _guard = SERIAL.lock().await;
    let db = Scratch::before_0015("up").await;
    let tables_before = db.tables();
    let roles_before = db.roles();
    assert!(!tables_before.contains(&"delivery_lookups".to_owned()));

    db.up();

    let mut expected = tables_before.clone();
    expected.push("delivery_lookups".to_owned());
    expected.sort();
    assert_eq!(db.tables(), expected, "exactly one table is added");
    assert_eq!(
        db.roles(),
        roles_before,
        "no role moves: delivery:read is granted to no one new"
    );

    let columns = sql(
        &db.url,
        "SELECT column_name || ' ' || data_type || ' ' || is_nullable \
           FROM information_schema.columns WHERE table_name = 'delivery_lookups' \
          ORDER BY ordinal_position",
    );
    assert_eq!(
        columns.lines().collect::<Vec<_>>(),
        [
            "id text NO",
            "actor_sub text NO",
            "actor_kind text NO",
            "actor_role text NO",
            "actor_app_id text YES",
            "key_kind text NO",
            "key_value text NO",
            "app_id_filter text YES",
            "result_count bigint NO",
            "created_at timestamp with time zone NO",
        ]
    );
}

#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn the_defaults_and_the_checks_are_the_models() {
    let _guard = SERIAL.lock().await;
    let db = Scratch::before_0015("checks").await;
    db.up();

    // A row the procedure would write: id and created_at come from the defaults.
    sql(
        &db.url,
        &insert(
            COLUMNS,
            "'u1', 'user', 'support', 'msisdn', 'hmac-sha256-v1:x', 2",
        ),
    );
    let defaults = sql(
        &db.url,
        "SELECT (id ~ '^c[0-9a-f]{22}$') || ' ' || (created_at > now() - interval '1 minute') \
           FROM delivery_lookups",
    );
    assert_eq!(defaults, "true true", "cs_cuid() and now()");

    let refusals: [(&str, &str, &str); 7] = [
        (
            "a key kind outside the enum",
            "'u1', 'user', 'support', 'number', 'k', 1",
            "key_kind_enum_check",
        ),
        (
            "an empty actor",
            "'', 'user', 'support', 'msisdn', 'k', 1",
            "actor_sub_length_check",
        ),
        (
            "an empty kind",
            "'u1', '', 'support', 'msisdn', 'k', 1",
            "actor_kind_length_check",
        ),
        (
            "an empty role",
            "'u1', 'user', '', 'msisdn', 'k', 1",
            "actor_role_length_check",
        ),
        (
            "an empty key",
            "'u1', 'user', 'support', 'msisdn', '', 1",
            "key_value_length_check",
        ),
        (
            "a negative count",
            "'u1', 'user', 'support', 'msisdn', 'k', -1",
            "result_count_check",
        ),
        (
            "an oversized actor",
            &format!("'{}', 'user', 'support', 'msisdn', 'k', 1", "s".repeat(256)),
            "actor_sub_length_check",
        ),
    ];
    for (what, values, constraint) in refusals {
        let error = refused(&db.url, &insert(COLUMNS, values));
        assert!(
            error.contains(constraint),
            "{what}: expected {constraint}, got {error}"
        );
    }
    // The enum takes all three kinds.
    for kind in ["message_id", "client_ref", "msisdn"] {
        sql(
            &db.url,
            &insert(
                COLUMNS,
                &format!("'u1', 'user', 'support', '{kind}', 'k', 0"),
            ),
        );
    }
}

#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn the_budgets_index_is_on_the_caller_and_the_time_newest_first() {
    let _guard = SERIAL.lock().await;
    let db = Scratch::before_0015("index").await;
    db.up();
    let definition = sql(
        &db.url,
        "SELECT indexdef FROM pg_indexes \
          WHERE tablename = 'delivery_lookups' AND indexname = 'delivery_lookups_actor_created_idx'",
    );
    assert!(
        definition.contains("(actor_sub, created_at DESC)"),
        "{definition}"
    );
}

#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn down_drops_the_table_and_nothing_else() {
    let _guard = SERIAL.lock().await;
    let db = Scratch::before_0015("down").await;
    let tables_before = db.tables();
    let roles_before = db.roles();
    db.up();
    db.down();
    assert_eq!(db.tables(), tables_before);
    assert_eq!(db.roles(), roles_before);
    let leftovers = sql(
        &db.url,
        "SELECT count(*) FROM pg_indexes WHERE tablename = 'delivery_lookups'",
    );
    assert_eq!(leftovers, "0");
}
