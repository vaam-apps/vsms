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
//! `up.sql` is then a no-op, so is the migration run twice (`IF NOT EXISTS`).
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

#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn the_due_list_of_one_endpoint_is_a_partial_index_that_down_sql_removes_alone() {
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
                apply(&scratch, &path);
            }
        }
    }

    // The definition Postgres reports back: the two columns in this order, partial
    // on the same states `webhook_due_idx` covers, and nothing else.
    let definition = index_definition(&scratch, INDEX);
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
            &scratch,
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
    apply(&scratch, &ours.join("down.sql"));
    assert_eq!(index_definition(&scratch, INDEX), "");
    for kept in ["webhook_due_idx", "webhook_attempts_lease_reclaim_idx"] {
        assert!(
            !index_definition(&scratch, kept).is_empty(),
            "{kept} is not this migration's to drop"
        );
    }

    // And going forward again lands the same index, however many times it runs.
    apply(&scratch, &ours.join("up.sql"));
    assert_eq!(index_definition(&scratch, INDEX), definition);
    apply(&scratch, &ours.join("up.sql"));
    assert_eq!(
        index_definition(&scratch, INDEX),
        definition,
        "a second run is a no-op (IF NOT EXISTS), not an error"
    );

    // The operator's way round the build's lock: build by hand, concurrently, then
    // let the migration find the index there. `psql -c` runs the statement outside a
    // transaction block, which `CONCURRENTLY` needs.
    apply(&scratch, &ours.join("down.sql"));
    must(&scratch, &["-c", RUNBOOK_STATEMENT]);
    assert_eq!(
        index_definition(&scratch, INDEX),
        definition,
        "the runbook's statement must build exactly the migration's index"
    );
    apply(&scratch, &ours.join("up.sql"));
    assert_eq!(index_definition(&scratch, INDEX), definition);
    assert!(
        RUNBOOK.contains(RUNBOOK_STATEMENT),
        "docs/runbooks/deployment.adoc no longer carries the statement this test runs"
    );

    must(
        &admin,
        &[
            "-c",
            &format!("DROP DATABASE IF EXISTS {SCRATCH} WITH (FORCE)"),
        ],
    );
}
