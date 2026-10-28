//! `0011_webhook_endpoint_due_idx`: the per-endpoint due list a keyless `hooks`
//! worker's claim reads (VS-8 follow-up M1). The index is partial and ordered
//! `(endpoint_id, next_attempt_at)`; its use by the claim, and the claim's
//! independence from a sealed backlog, are proved in `sms-worker`'s
//! `hooks_live_postgres.rs`. This file proves the SQL: what the migration creates,
//! and that `down.sql` takes exactly that away and nothing the global due list or
//! the lease reclaim need.
//!
//! It also proves the way around the build's lock: the statement
//! `docs/runbooks/deployment.adoc` tells an operator to run by hand beforehand
//! (`CREATE INDEX CONCURRENTLY IF NOT EXISTS`) lands the same definition, and
//! `up.sql` then runs no `CREATE INDEX`, so is the migration run twice. And it
//! proves what `up.sql` refuses: an index of that name that is *invalid* (the
//! leftover of an interrupted `CONCURRENTLY` build, over which a bare
//! `CREATE INDEX IF NOT EXISTS` reports "already exists, skipping" and succeeds)
//! or that has another definition, and that over a valid one it takes no lock on
//! `webhook_attempts` (a bare `IF NOT EXISTS` queues for the `SHARE` lock first).
//!
//! Plain `psql`, like the harness's own migration step, against a scratch database
//! built from every migration in order.

use std::path::{Path, PathBuf};
use std::process::Command;

const SCRATCH: &str = "vsms_mig0011_scratch";
const INDEX: &str = "webhook_endpoint_due_idx";

/// The statement the deployment runbook gives an operator for a large table, to run
/// before the deploy. The test runs it, and checks that the runbook still says it.
const RUNBOOK_STATEMENT: &str = "CREATE INDEX CONCURRENTLY IF NOT EXISTS webhook_endpoint_due_idx \
     ON webhook_attempts (endpoint_id, next_attempt_at) WHERE state IN ('pending','failed')";
const RUNBOOK: &str = include_str!("../../../../docs/runbooks/deployment.adoc");

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

fn must(url: &str, args: &[&str]) -> String {
    let output = Command::new("psql")
        .arg(url)
        .args(["-v", "ON_ERROR_STOP=1", "-q", "-At"])
        .args(args)
        .output()
        .expect("spawning `psql` — is the postgresql-client package installed?");
    assert!(
        output.status.success(),
        "psql {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn apply(url: &str, file: &Path) {
    must(url, &["-f", file.to_str().expect("a UTF-8 path")]);
}

/// The definition Postgres reports for an index, or empty if there is none.
fn index_definition(url: &str, index: &str) -> String {
    must(
        url,
        &[
            "-c",
            &format!("SELECT indexdef FROM pg_indexes WHERE indexname = '{index}'"),
        ],
    )
}

/// A scratch database built from every migration in order, and the directory of
/// this migration. Dropped and rebuilt if a failed run left one behind.
struct Scratch {
    admin: String,
    url: String,
    ours: PathBuf,
}

impl Scratch {
    async fn build(name: &str) -> Self {
        let base = sms_test_support::database_url().await;
        let admin = with_database(&base, "postgres");
        let url = with_database(&base, name);
        must(
            &admin,
            &[
                "-c",
                &format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
            ],
        );
        must(&admin, &["-c", &format!("CREATE DATABASE {name}")]);

        let mut dirs: Vec<PathBuf> = std::fs::read_dir(migrations_root())
            .expect("reading the migrations")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        let ours = dirs
            .iter()
            .find(|dir| dir.ends_with("0011_webhook_endpoint_due_idx"))
            .expect("0011_webhook_endpoint_due_idx")
            .clone();
        for dir in &dirs {
            for file in ["up.pre.sql", "up.sql"] {
                let path = dir.join(file);
                if path.exists() {
                    apply(&url, &path);
                }
            }
        }
        Self { admin, url, ours }
    }

    fn drop_database(&self, name: &str) {
        must(
            &self.admin,
            &[
                "-c",
                &format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
            ],
        );
    }
}

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_due_list_of_one_endpoint_is_a_partial_index_that_down_sql_removes_alone() {
    let scratch_db = Scratch::build(SCRATCH).await;
    let (scratch, ours) = (&scratch_db.url, &scratch_db.ours);

    // The definition Postgres reports back: the two columns in this order, partial
    // on the same states `webhook_due_idx` covers, and nothing else.
    let definition = index_definition(scratch, INDEX);
    assert!(
        definition
            .contains("ON public.webhook_attempts USING btree (endpoint_id, next_attempt_at)"),
        "{definition}"
    );
    assert!(
        definition.contains("WHERE (state = ANY (ARRAY['pending'::text, 'failed'::text]))"),
        "{definition}"
    );
    assert_eq!(
        must(
            scratch,
            &[
                "-c",
                &format!(
                    "SELECT count(*) FROM pg_indexes WHERE indexname = '{INDEX}' \
                     AND indexdef LIKE 'CREATE UNIQUE%'"
                )
            ]
        ),
        "0",
        "an ordinary index: a unique one would refuse a second attempt at one time"
    );

    // Rolling back removes it and nothing else.
    apply(scratch, &ours.join("down.sql"));
    assert_eq!(index_definition(scratch, INDEX), "");
    for kept in ["webhook_due_idx", "webhook_attempts_lease_reclaim_idx"] {
        assert!(
            !index_definition(scratch, kept).is_empty(),
            "{kept} is not this migration's to drop"
        );
    }

    // And going forward again lands the same index, however many times it runs.
    apply(scratch, &ours.join("up.sql"));
    assert_eq!(index_definition(scratch, INDEX), definition);
    apply(scratch, &ours.join("up.sql"));
    assert_eq!(
        index_definition(scratch, INDEX),
        definition,
        "a second run over a valid index leaves it alone and is not an error"
    );

    // The operator's way round the build's lock: build by hand, concurrently, then
    // let the migration find the index there. `psql -c` runs the statement outside a
    // transaction block, which `CONCURRENTLY` needs.
    apply(scratch, &ours.join("down.sql"));
    must(scratch, &["-c", RUNBOOK_STATEMENT]);
    assert_eq!(
        index_definition(scratch, INDEX),
        definition,
        "the runbook's statement must build exactly the migration's index"
    );
    apply(scratch, &ours.join("up.sql"));
    assert_eq!(index_definition(scratch, INDEX), definition);
    assert!(
        RUNBOOK.contains(RUNBOOK_STATEMENT),
        "docs/runbooks/deployment.adoc no longer carries the statement this test runs"
    );

    scratch_db.drop_database(SCRATCH);
}

/// Runs `args` with `psql` and returns what it printed on stderr, failing the test
/// if it succeeded: for a statement that must be refused.
fn must_fail(url: &str, args: &[&str]) -> String {
    let output = Command::new("psql")
        .arg(url)
        .args(["-v", "ON_ERROR_STOP=1", "-q", "-At"])
        .args(args)
        .output()
        .expect("spawning `psql` — is the postgresql-client package installed?");
    assert!(
        !output.status.success(),
        "psql {args:?} succeeded and was meant to be refused: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Whether Postgres considers the index usable by queries.
fn index_is_valid(url: &str) -> String {
    must(
        url,
        &[
            "-c",
            &format!("SELECT indisvalid FROM pg_index WHERE indexrelid = '{INDEX}'::regclass"),
        ],
    )
}

/// Review round 7: a migration must not succeed over an index it cannot vouch for,
/// and must not take a lock it does not need.
///
/// - After a failed or interrupted `CREATE INDEX CONCURRENTLY` the index exists and
///   is `INVALID`. `CREATE INDEX IF NOT EXISTS` printed "already exists, skipping"
///   over it and the migration succeeded, with the claim's per-endpoint read left
///   to a plan no index serves. `up.sql` fails instead, naming the runbook section,
///   and the index is left as it was for the operator to drop and rebuild.
/// - The same for a valid index of that name with another definition.
/// - `CREATE INDEX IF NOT EXISTS` takes the `SHARE` lock before it checks. Over an
///   index that is there, `up.sql` runs no `CREATE INDEX`: with a writer holding
///   `ROW EXCLUSIVE` on the table (an uncommitted UPDATE would do the same), it
///   still finishes inside a one-second `lock_timeout`.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn up_sql_refuses_an_invalid_index_and_another_definition_and_takes_no_lock_over_a_valid_one()
{
    const NAME: &str = "vsms_mig0011_invalid_scratch";
    let scratch_db = Scratch::build(NAME).await;
    let (scratch, ours) = (&scratch_db.url, &scratch_db.ours);
    let up = ours.join("up.sql");
    let up = up.to_str().expect("a UTF-8 path");
    let definition = index_definition(scratch, INDEX);
    assert_eq!(index_is_valid(scratch), "t");

    // No lock over a valid index: a writer holds ROW EXCLUSIVE, which conflicts with
    // the SHARE lock a CREATE INDEX asks for, and the migration is not made to wait.
    let mut writer = Command::new("psql")
        .arg(scratch)
        .args(["-X", "-q", "-At", "-c"])
        .arg(
            "BEGIN; LOCK TABLE webhook_attempts IN ROW EXCLUSIVE MODE; SELECT pg_sleep(30); COMMIT;",
        )
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawning the writer");
    let mut held = false;
    for _ in 0..50 {
        let granted = must(
            scratch,
            &[
                "-c",
                "SELECT count(*) FROM pg_locks WHERE relation = 'webhook_attempts'::regclass \
                 AND mode = 'RowExclusiveLock' AND granted",
            ],
        );
        if granted != "0" {
            held = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(held, "the writer never took its lock");
    let started = std::time::Instant::now();
    let output = Command::new("psql")
        .arg(scratch)
        .env("PGOPTIONS", "-c lock_timeout=1000")
        .args(["-v", "ON_ERROR_STOP=1", "-X", "-q", "-f", up])
        .output()
        .expect("spawning `psql`");
    writer.kill().expect("stopping the writer");
    writer.wait().expect("reaping the writer");
    assert!(
        output.status.success(),
        "up.sql waited for a lock over a valid index (after {:?}): {}",
        started.elapsed(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(index_definition(scratch, INDEX), definition);

    // An invalid index, as an interrupted CONCURRENTLY build leaves it.
    must(
        scratch,
        &[
            "-c",
            &format!(
                "UPDATE pg_index SET indisvalid = false WHERE indexrelid = '{INDEX}'::regclass"
            ),
        ],
    );
    assert_eq!(index_is_valid(scratch), "f");
    let refused = must_fail(scratch, &["-f", up]);
    assert!(
        refused.contains("INVALID") && refused.contains("Before deploying: the webhook_attempts"),
        "the refusal names the cause and the runbook section: {refused}"
    );
    assert_eq!(
        index_is_valid(scratch),
        "f",
        "the migration leaves the invalid index as it found it"
    );
    assert!(
        RUNBOOK.contains("Before deploying: the `webhook_attempts` index of migration 0011"),
        "the section the refusal names is gone from docs/runbooks/deployment.adoc"
    );

    // The repair the refusal asks for lets it through.
    must(scratch, &["-c", &format!("DROP INDEX {INDEX}")]);
    apply(scratch, ours.join("up.sql").as_path());
    assert_eq!(index_definition(scratch, INDEX), definition);
    assert_eq!(index_is_valid(scratch), "t");

    // A valid index of that name that is not this one.
    must(
        scratch,
        &[
            "-c",
            &format!("DROP INDEX {INDEX}"),
            "-c",
            &format!(
                "CREATE INDEX {INDEX} ON webhook_attempts (next_attempt_at, endpoint_id) \
                 WHERE state IN ('pending','failed')"
            ),
        ],
    );
    let refused = must_fail(scratch, &["-f", up]);
    assert!(refused.contains("another definition"), "{refused}");

    scratch_db.drop_database(NAME);
}
