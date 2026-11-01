//! `0014_optout_read_scope`: the built-in `auditor` gains `optout:read`, on a
//! database that already ran `0001`..`0013`.
//!
//! What this file holds the migration to:
//!
//! - **Exactly the owner's change.** Before the migration `auditor` holds what
//!   `0002_bootstrap` seeded (`0013` leaves it alone); after it, the same set
//!   plus `optout:read` and nothing else, `version` one higher. Every other row
//!   (`owner`, `admin`, `operator`, `developer`, `support`, `pending`, a custom
//!   role) is byte-identical, `updated_at` included: `owner`, `admin`,
//!   `operator` and `support` already hold `optout:manage`, which reads
//!   opt-outs, and are not given `optout:read` as well.
//! - **Only the built-in `auditor` is touched.** The same permissions on a row
//!   that is not built in (the flag cleared on the auditor row itself, since
//!   the key is unique), on a built-in-flagged row under another key, and on a
//!   custom role holding `sms:read`: all untouched. A database with no `auditor`
//!   row at all migrates without a row changed.
//! - **An `auditor` that already holds it is not bumped.** At the start, in the
//!   middle or at the end of the set, packed or not: no row, `version` or
//!   `updated_at` moves.
//! - **The addition is robust to the format.** A set that is not sentinel-packed
//!   (tabs, newlines, a doubled space, no leading or trailing space, empty)
//!   gains the token and comes out packed; a longer permission it is a prefix of
//!   (`optout:readonly`) or a suffix of (`xoptout:read`), and `optout:manage`,
//!   never count as holding it.
//! - **Idempotent.** The second and third runs change no row at all (the whole
//!   `roles` table, `updated_at` included, is compared).
//! - **`down.sql`** takes the token back, once, and nothing else with it.
//!
//! Plain `psql`, like `role_read_scope_migration_live_postgres.rs`, which this
//! file is modelled on: the point is the SQL, and the "before" state is the
//! database as `0013` leaves it. Each migration is applied the way
//! `sms-migrate` does, in one transaction (`psql -1`), on a scratch database of
//! its own.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The packed permission set `0002_bootstrap` seeds `auditor` with, which
/// `0013_role_read_scope` leaves as it is.
const OLD_AUDITOR: &str = " sms:read app:read provider:read route:read job:read worker:read dashboard:read delivery:read audit:read ";
const NEW_AUDITOR: &str = " sms:read app:read provider:read route:read job:read worker:read dashboard:read delivery:read audit:read optout:read ";

/// The directory name of the migration this file is about.
const THIS: &str = "0014_optout_read_scope";

/// One test at a time: they share the scratch databases' prefix and one server.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

/// Applies one migration file the way `sms-migrate` does: in one transaction.
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
        .expect("0014_optout_read_scope exists")
}

/// A scratch database migrated through `0013`, dropped when the test ends.
struct Scratch {
    admin: String,
    name: String,
    url: String,
}

impl Scratch {
    async fn before_0014(name: &str) -> Self {
        let base = sms_test_support::database_url().await;
        let admin = with_database(&base, "postgres");
        let name = format!("vsms_mig0014_{name}");
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

    /// The packed permissions of the role keyed `key`, with its version.
    fn role(&self, key: &str) -> (String, i64) {
        let row = sql(
            &self.url,
            &format!("SELECT '[' || permissions || ']|' || version FROM roles WHERE key = '{key}'"),
        );
        // Bracketed, because `psql` output is trimmed and a packed set is
        // nothing but spaces at its ends.
        let (permissions, version) = row
            .rsplit_once("]|")
            .and_then(|(permissions, version)| Some((permissions.strip_prefix('[')?, version)))
            .unwrap_or_else(|| panic!("no role {key}: {row:?}"));
        (permissions.to_owned(), version.parse().expect("a version"))
    }

    /// Every column of every role, in key order: two equal dumps are two equal
    /// tables, `updated_at` (which the `touch` trigger moves on any UPDATE) included.
    fn dump(&self) -> String {
        sql(
            &self.url,
            "SELECT key, builtin, label, coalesce(description, '<null>'), \
                    '[' || permissions || ']', version, updated_at \
               FROM roles ORDER BY key",
        )
    }

    fn set_permissions(&self, key: &str, permissions: &str) {
        sql(
            &self.url,
            &format!(
                "UPDATE roles SET permissions = '{permissions}', version = 0 WHERE key = '{key}'"
            ),
        );
    }

    fn exists(&self, key: &str) -> bool {
        sql(
            &self.url,
            &format!("SELECT count(*) FROM roles WHERE key = '{key}'"),
        ) == "1"
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

/// The upgrade: the seed in, the owner's decision out, nothing else moved.
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn the_auditor_gains_optout_read_and_no_other_row_moves() {
    let _guard = SERIAL.lock().await;
    let db = Scratch::before_0014("upgrade").await;
    assert_eq!(
        db.role("auditor"),
        (OLD_AUDITOR.to_owned(), 0),
        "the seed 0013 left"
    );

    // Rows that must not move: a person-made role with sms:read, a role holding
    // the same permissions as the auditor, and a built-in-flagged row under
    // another key.
    sql(
        &db.url,
        "INSERT INTO roles (key, label, description, builtin, permissions, version) VALUES \
           ('custom_reader', 'Custom reader', NULL, false, ' sms:read app:read ', 0), \
           ('auditor_copy',  'Auditor copy',  NULL, false, ' sms:read audit:read ', 3), \
           ('flagged_other', 'Flagged other', NULL, true,  ' sms:read ', 0)",
    );
    let before = db.dump();

    db.up();
    let after = db.dump();

    assert_eq!(
        db.role("auditor"),
        (NEW_AUDITOR.to_owned(), 1),
        "auditor: optout:read appended, the rest as it was, one version up"
    );
    let changed: Vec<&str> = after
        .lines()
        .filter(|line| !before.lines().any(|b| b == *line))
        .map(|line| line.split('|').next().unwrap_or(line))
        .collect();
    assert_eq!(
        changed,
        ["auditor"],
        "exactly the auditor differs\nbefore:\n{before}\nafter:\n{after}"
    );
    for untouched in [
        "owner",
        "admin",
        "operator",
        "developer",
        "support",
        "pending",
        "custom_reader",
        "auditor_copy",
        "flagged_other",
    ] {
        let line = |dump: &str| {
            dump.lines()
                .find(|line| line.starts_with(&format!("{untouched}|")))
                .map(str::to_owned)
        };
        assert!(line(&before).is_some(), "{untouched} exists");
        assert_eq!(line(&before), line(&after), "{untouched} must not move");
    }
    for manager in ["owner", "admin", "operator", "support"] {
        let (permissions, _) = db.role(manager);
        assert!(
            permissions.contains(" optout:manage ") && !permissions.contains(" optout:read "),
            "{manager} manages opt-outs and is not given optout:read: {permissions:?}"
        );
    }
    for blind in ["developer", "pending"] {
        let (permissions, _) = db.role(blind);
        assert!(
            !permissions.contains("optout:"),
            "{blind} holds no opt-out permission: {permissions:?}"
        );
    }
}

/// Only the built-in row keyed `auditor` is touched: not one that lost its
/// built-in flag (an owner took it over), and not a missing one.
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn only_the_builtin_auditor_row_is_touched() {
    let _guard = SERIAL.lock().await;
    let db = Scratch::before_0014("builtin").await;
    sql(
        &db.url,
        "UPDATE roles SET builtin = false WHERE key = 'auditor'",
    );
    let before = db.dump();
    db.up();
    assert_eq!(
        before,
        db.dump(),
        "an auditor that is not built in is theirs"
    );

    sql(&db.url, "DELETE FROM roles WHERE key = 'auditor'");
    let before = db.dump();
    db.up();
    assert_eq!(before, db.dump(), "no auditor row, no row changed");
    assert!(!db.exists("auditor"), "the migration does not create one");
}

/// The second run, and a run on an `auditor` an owner already edited to hold
/// the token, change nothing: not a permission, not a version, not `updated_at`.
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn running_it_twice_changes_nothing_the_second_time() {
    let _guard = SERIAL.lock().await;
    let db = Scratch::before_0014("twice").await;
    db.up();
    let once = db.dump();
    db.up();
    assert_eq!(once, db.dump(), "the second run touched a row");
    db.up();
    assert_eq!(once, db.dump(), "the third run touched a row");
    assert_eq!(db.role("auditor").1, 1, "one bump, not three");

    // An auditor already holding the token, at the start, in the middle or at
    // the end of the set, packed or not, is not a bump either.
    for held in [
        " optout:read sms:read ",
        " sms:read optout:read audit:read ",
        " sms:read audit:read optout:read ",
        " optout:read ",
        "optout:read",
        "sms:read\toptout:read\taudit:read",
        "  sms:read   optout:read  ",
    ] {
        db.set_permissions("auditor", held);
        let edited = db.dump();
        db.up();
        assert_eq!(edited, db.dump(), "an auditor holding {held:?} moved");
        assert_eq!(db.role("auditor"), (held.to_owned(), 0), "from {held:?}");
    }
}

/// The addition on every shape of set the Rust side would read the same
/// (`split_whitespace`): the token is appended once, the set comes out packed
/// with no double space, no other permission is eaten, and a longer permission
/// the token is a prefix or suffix of is not mistaken for it.
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn the_addition_is_packed_whatever_the_set_looked_like() {
    let _guard = SERIAL.lock().await;
    let db = Scratch::before_0014("format").await;
    let cases: [(&str, &str); 14] = [
        (" sms:read ", " sms:read optout:read "),
        (" ", " optout:read "),
        ("", " optout:read "),
        (" \t ", " optout:read "),
        // Not sentinel-packed: appended all the same, and rewritten packed.
        ("sms:read", " sms:read optout:read "),
        ("sms:read audit:read", " sms:read audit:read optout:read "),
        ("sms:read\taudit:read", " sms:read audit:read optout:read "),
        (
            " sms:read\naudit:read\n",
            " sms:read audit:read optout:read ",
        ),
        (
            "  sms:read   audit:read  ",
            " sms:read audit:read optout:read ",
        ),
        ("\tsms:read\t", " sms:read optout:read "),
        // Look-alikes are not the token.
        (" optout:readonly ", " optout:readonly optout:read "),
        (" xoptout:read ", " xoptout:read optout:read "),
        (
            " optout:reader sms:read ",
            " optout:reader sms:read optout:read ",
        ),
        // `optout:manage` reads for a person, but it is not `optout:read`: a
        // built-in auditor an owner gave it keeps it and is given the other too.
        (" optout:manage ", " optout:manage optout:read "),
    ];
    for (input, expected) in cases {
        db.set_permissions("auditor", input);
        db.up();
        assert_eq!(
            db.role("auditor"),
            (expected.to_owned(), 1),
            "from {input:?}"
        );
        let packed = db.role("auditor").0;
        assert!(!packed.contains("  "), "a double space from {input:?}");
        assert!(
            packed.starts_with(' ') && packed.ends_with(' '),
            "not packed from {input:?}"
        );
        // A second run on the result is a no-op.
        let once = db.dump();
        db.up();
        assert_eq!(once, db.dump(), "a second run moved {input:?}");
    }
}

/// `down.sql` takes the token back (once) and nothing with it, and leaves a row
/// that never held it alone.
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn down_takes_the_token_back_and_nothing_else() {
    let _guard = SERIAL.lock().await;
    let db = Scratch::before_0014("down").await;
    db.up();
    assert_eq!(db.role("auditor"), (NEW_AUDITOR.to_owned(), 1));
    db.down();
    assert_eq!(
        db.role("auditor"),
        (OLD_AUDITOR.to_owned(), 2),
        "up then down is two bumps and the old set"
    );
    db.down();
    assert_eq!(db.role("auditor").1, 2, "a second down changes nothing");

    // The token alone leaves the empty packed set; a look-alike stays.
    db.set_permissions("auditor", " optout:read ");
    db.down();
    assert_eq!(db.role("auditor").0, " ");
    db.set_permissions(
        "auditor",
        " optout:readonly xoptout:read optout:read optout:manage ",
    );
    db.down();
    assert_eq!(
        db.role("auditor").0,
        " optout:readonly xoptout:read optout:manage "
    );
    let kept = db.dump();
    db.down();
    assert_eq!(kept, db.dump());
}
