//! `0013_role_read_scope`: `developer` and `support` lose `sms:read`, and a
//! zero-permission `pending` role appears, on a database that already ran
//! `0001`..`0012` (the upgrade) and on one that never saw the old seed.
//!
//! What this file holds the migration to:
//!
//! - **Exactly the owner's change.** Before the migration `developer` and
//!   `support` hold `sms:read` (the `0002_bootstrap` seed) and there is no
//!   `pending`; after it they hold what they held, minus `sms:read` and nothing
//!   else, each `version` is one higher, and `pending` is built in and holds
//!   the empty packed set `' '`. `owner`, `admin`, `operator` and `auditor` are
//!   byte-identical, `updated_at` included.
//! - **A custom role is never touched.** A role an owner created (not built in),
//!   whatever it is called and holds, and a built-in-flagged row under any other
//!   key keep `sms:read`.
//! - **A `pending` role an owner made is never kept with permissions.** One that
//!   holds any makes the migration refuse and roll back whole (`jit_role: pending`
//!   would hand them to every first sign-in), nothing is overwritten; an empty
//!   one is accepted and left as it is.
//! - **The removal is robust to the format.** The permission at the start,
//!   middle or end of the set, alone, repeated, beside a longer permission it is
//!   a prefix of (`sms:readonly`) or a suffix of (`xsms:read`), and in a set that
//!   is not sentinel-packed (tabs, a doubled space, no leading or trailing
//!   space), which the Rust side reads with `split_whitespace` all the same:
//!   never a double space, never a dangling separator, never a different
//!   permission eaten, and the result is always packed.
//! - **Idempotent.** The second run changes no row at all (the whole `roles`
//!   table, `updated_at` included, is compared).
//! - **`down.sql`** gives the permission back and removes `pending`, except
//!   while a user holds it.
//!
//! Plain `psql`, like the harness's own migration step and
//! `restore_openid_provider_live_postgres.rs`: the point is the SQL, and the
//! "before" state is the old seed. Each migration is applied the way
//! `sms-migrate` does, in one transaction (`psql -1`), on a scratch database
//! of its own.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The packed permission set `0002_bootstrap` seeds `developer` with.
const OLD_DEVELOPER: &str = " app:read webhook:manage sms:read sms:send ";
const OLD_SUPPORT: &str = " sms:read optout:manage delivery:read ";
const NEW_DEVELOPER: &str = " app:read webhook:manage sms:send ";
const NEW_SUPPORT: &str = " optout:manage delivery:read ";

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
        .find(|dir| dir.ends_with("0013_role_read_scope"))
        .expect("0013_role_read_scope exists")
}

/// A scratch database migrated through `0012`, dropped when the test ends.
struct Scratch {
    admin: String,
    name: String,
    url: String,
}

impl Scratch {
    async fn before_0013(name: &str) -> Self {
        let base = sms_test_support::database_url().await;
        let admin = with_database(&base, "postgres");
        let name = format!("vsms_mig0013_{name}");
        let url = with_database(&base, &name);
        drop_database(&admin, &name);
        must(&admin, &["-c", &format!("CREATE DATABASE {name}")]);
        let scratch = Self { admin, name, url };
        for dir in migration_dirs()
            .into_iter()
            .filter(|dir| !dir.ends_with("0013_role_read_scope"))
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

    /// The same, expected to be refused: `psql`'s stderr.
    fn up_refused(&self) -> String {
        let file = this_migration().join("up.sql");
        let (ok, _, stderr) = psql(
            &self.url,
            &["-1", "-f", file.to_str().expect("a UTF-8 path")],
        );
        assert!(!ok, "the migration was accepted, but must be refused");
        stderr
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

/// The upgrade: the old seed in, the owner's decision out, nothing else moved.
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn developer_and_support_lose_sms_read_and_pending_appears() {
    let db = Scratch::before_0013("upgrade").await;
    assert_eq!(
        db.role("developer"),
        (OLD_DEVELOPER.to_owned(), 0),
        "the old seed"
    );
    assert_eq!(db.role("support"), (OLD_SUPPORT.to_owned(), 0));
    assert!(!db.exists("pending"), "no pending before 0013");

    // Rows that must not move: the four reading roles, and three a person made.
    sql(
        &db.url,
        "INSERT INTO roles (key, label, description, builtin, permissions, version) VALUES \
           ('custom_reader', 'Custom reader', NULL, false, ' sms:read app:read ', 0), \
           ('support_copy',  'Support copy',  NULL, false, ' sms:read optout:manage delivery:read ', 3), \
           ('flagged_other', 'Flagged other', NULL, true,  ' sms:read ', 0)",
    );
    let before = db.dump();

    db.up();
    let after = db.dump();

    assert_eq!(
        db.role("developer"),
        (NEW_DEVELOPER.to_owned(), 1),
        "developer: sms:read gone, the rest as it was, one version up"
    );
    assert_eq!(db.role("support"), (NEW_SUPPORT.to_owned(), 1));
    let pending = sql(
        &db.url,
        "SELECT builtin || '|[' || permissions || ']|' || version || '|' || label \
           FROM roles WHERE key = 'pending'",
    );
    assert_eq!(pending, "true|[ ]|0|Pending");

    let changed: Vec<&str> = after
        .lines()
        .filter(|line| !before.lines().any(|b| b == *line))
        .map(|line| line.split('|').next().unwrap_or(line))
        .collect();
    assert_eq!(
        changed,
        ["developer", "pending", "support"],
        "exactly the owner's three rows differ\nbefore:\n{before}\nafter:\n{after}"
    );
    for untouched in [
        "owner",
        "admin",
        "operator",
        "auditor",
        "custom_reader",
        "support_copy",
        "flagged_other",
    ] {
        let line = |dump: &str| {
            dump.lines()
                .find(|line| line.starts_with(&format!("{untouched}|")))
                .map(str::to_owned)
        };
        assert_eq!(line(&before), line(&after), "{untouched} must not move");
    }
    assert!(
        after.contains("custom_reader|f|") && db.role("custom_reader").0.contains(" sms:read "),
        "a custom role keeps sms:read"
    );
}

/// The second run, and a run on a database an owner already edited, change
/// nothing: not a permission, not a version, not `updated_at`.
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn running_it_twice_changes_nothing_the_second_time() {
    let db = Scratch::before_0013("twice").await;
    db.up();
    let once = db.dump();
    db.up();
    assert_eq!(once, db.dump(), "the second run touched a row");
    db.up();
    assert_eq!(once, db.dump(), "the third run touched a row");

    // A built-in role an owner already edited to lack the permission is not a
    // second bump either.
    db.set_permissions("operator", " sms:send ");
    let edited = db.dump();
    db.up();
    assert_eq!(edited, db.dump());
}

/// The packed format `' a b c '` loses the permission and one neighbouring
/// space, whatever the position, and never anything longer or shorter. A set the
/// Rust side would read the same (`split_whitespace`) but that is not packed
/// (tabs, newlines, doubled spaces, no sentinel) loses it too, and comes out
/// packed.
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn the_removal_leaves_no_double_space_and_eats_no_other_permission() {
    let db = Scratch::before_0013("format").await;
    let cases: [(&str, &str, i64); 21] = [
        (" sms:read ", " ", 1),
        (" sms:read app:read ", " app:read ", 1),
        (" app:read sms:read ", " app:read ", 1),
        (" app:read sms:read sms:send ", " app:read sms:send ", 1),
        (" sms:read sms:read x:y ", " x:y ", 1),
        (" x:y sms:read sms:read ", " x:y ", 1),
        (" xsms:read sms:read ", " xsms:read ", 1),
        (" sms:readonly sms:read ", " sms:readonly ", 1),
        // Not sentinel-packed: removed all the same, and rewritten packed.
        ("sms:read app:read", " app:read ", 1),
        ("app:read sms:read", " app:read ", 1),
        ("sms:read", " ", 1),
        ("app:read\tsms:read\tsms:send", " app:read sms:send ", 1),
        (" sms:read\tapp:read ", " app:read ", 1),
        (" app:read\nsms:read\n", " app:read ", 1),
        ("  app:read   sms:read  x:y ", " app:read x:y ", 1),
        ("\tsms:read\t", " ", 1),
        // Nothing to remove: untouched (as written), and not a version bump.
        (" sms:readonly app:read ", " sms:readonly app:read ", 0),
        (" xsms:read ", " xsms:read ", 0),
        (" app:read ", " app:read ", 0),
        ("app:read\tx:y", "app:read\tx:y", 0),
        (" ", " ", 0),
    ];
    for (input, expected, version) in cases {
        db.set_permissions("developer", input);
        db.up();
        assert_eq!(
            db.role("developer"),
            (expected.to_owned(), version),
            "from {input:?}"
        );
        assert!(
            !db.role("developer").0.contains("  "),
            "a double space from {input:?}"
        );
    }
}

/// A `pending` role holding any permission was made by an owner, and
/// `jit_role: pending` would hand it to every first sign-in: the migration
/// refuses (and its transaction rolls back, so the `UPDATE` before it is undone
/// too), whatever the role's flags, and overwrites nothing. An empty one, from a
/// tab-and-space blank to the packed `' '`, is accepted and left as it is.
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn a_pending_role_that_holds_permissions_stops_the_migration_and_an_empty_one_does_not() {
    let db = Scratch::before_0013("pending_exists").await;
    sql(
        &db.url,
        "INSERT INTO roles (key, label, description, builtin, permissions, version) VALUES \
           ('pending', 'My pending', 'made by hand', false, ' app:read ', 4)",
    );
    let before = db.dump();
    let stderr = db.up_refused();
    assert!(
        stderr.contains("a role keyed pending already exists and holds permissions"),
        "{stderr}"
    );
    assert_eq!(before, db.dump(), "a refused migration changes nothing");
    assert_eq!(
        db.role("developer"),
        (OLD_DEVELOPER.to_owned(), 0),
        "the UPDATE ahead of the refusal rolled back"
    );

    // Built in or not, a role holding anything is refused.
    sql(
        &db.url,
        "UPDATE roles SET builtin = true WHERE key = 'pending'",
    );
    db.up_refused();

    // Emptied by its owner, the same database migrates, and the row is theirs.
    for blank in [" ", "", " \t "] {
        db.set_permissions("pending", blank);
        let kept = db.dump();
        db.up();
        let line = |dump: &str| {
            dump.lines()
                .find(|l| l.starts_with("pending|"))
                .map(str::to_owned)
        };
        assert_eq!(
            line(&kept),
            line(&db.dump()),
            "an empty pending is kept as it is"
        );
        assert_eq!(db.role("developer").0, NEW_DEVELOPER, "the rest ran");
        // Back to the old seed for the next blank.
        db.set_permissions("developer", OLD_DEVELOPER);
        db.set_permissions("support", OLD_SUPPORT);
    }
}

/// `down.sql` gives the permission back (once) and removes `pending`, unless a
/// person holds it (`users.role_key` references it).
#[tokio::test]
#[ignore = "needs a live Postgres and psql - see module docs"]
async fn down_restores_the_permission_and_removes_pending_unless_it_is_held() {
    let db = Scratch::before_0013("down").await;
    db.up();
    db.down();
    assert_eq!(
        db.role("developer").0,
        " app:read webhook:manage sms:send sms:read "
    );
    assert_eq!(
        db.role("support").0,
        " optout:manage delivery:read sms:read "
    );
    assert_eq!(db.role("developer").1, 2, "up then down is two bumps");
    assert!(!db.exists("pending"));
    db.down();
    assert_eq!(db.role("developer").1, 2, "a second down changes nothing");

    db.up();
    sql(
        &db.url,
        "INSERT INTO users (id, subject, email, display_name, role_key, version, created_at, updated_at) \
         VALUES ('u1', 's1', 'p@example.test', 'P', 'pending', 0, now(), now())",
    );
    db.down();
    assert!(db.exists("pending"), "a role a user holds is not deleted");
}
