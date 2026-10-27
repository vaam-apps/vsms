//! `0001_init` matches what `cratestack migrate diff` actually produces
//! from the current `schemas/vsms.cstack`.
//!
//! Port of the deleted `ci/assert-migrations-current.sh` (#204). Migrations
//! here are regenerated *wholesale* — there is no committed
//! `schema.snapshot.json` baseline (`AGENTS.md`'s "Regenerating
//! migrations" section) — so a stale `0001_init` applies fine to an empty
//! database and compiles fine against `include_server_schema!` while
//! silently missing whatever a schema edit added, until some live suite
//! happens to touch the affected column. Two concurrent PRs each
//! regenerating `0001_init` from a different base is the concrete way this
//! happens: Git merges the result as ordinary text, and a non-overlapping
//! hunk merges silently wrong.
//!
//! Assumes a `cratestack` binary is already on `PATH`, version-locked to
//! the pin this module reads via [`crate::cratestack_pin`] — same division
//! of labour the deleted script had with `ci/apply-migrations.sh` assuming
//! `psql` is already installed by a preceding CI step.
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SCHEMA: &str = "schemas/vsms.cstack";
const COMMITTED_DIR: &str = "backends/migrations/postgres/0001_init";

/// A migration that rewrites statements of the already-applied `0001_init`.
///
/// `0001_init` is regenerated wholesale only while no database has applied
/// it. A deployed one has, so a schema change that alters an emitted
/// statement ships as a forward migration instead, and this check compares
/// `0001_init` to what the generator emits *minus* what that migration
/// rewrites: each `(current, applied)` pair is a statement the generator
/// emits now and the line `0001_init` still carries. `LATER_UP` must contain
/// every `current` line, so the pair cannot outlive the migration. Delete
/// the pair and regenerate `0001_init` whenever it is squashed.
const LATER_UP: &str = "backends/migrations/postgres/0004_import_to_imported/up.sql";
const REWRITTEN_BY_LATER: [(&str, &str); 2] = [
    (
        "ALTER TABLE consent_records ADD CONSTRAINT consent_records_channel_enum_check CHECK (channel IN ('web_form', 'api', 'ivr', 'paper_form', 'verbal', 'sms_keyword', 'imported', 'admin'));",
        "ALTER TABLE consent_records ADD CONSTRAINT consent_records_channel_enum_check CHECK (channel IN ('web_form', 'api', 'ivr', 'paper_form', 'verbal', 'sms_keyword', 'import', 'admin'));",
    ),
    (
        "ALTER TABLE opt_outs ADD CONSTRAINT opt_outs_source_enum_check CHECK (source IN ('inbound_stop', 'admin', 'imported', 'operator'));",
        "ALTER TABLE opt_outs ADD CONSTRAINT opt_outs_source_enum_check CHECK (source IN ('inbound_stop', 'admin', 'import', 'operator'));",
    ),
];

/// Tables that exist in `schemas/vsms.cstack` but whose DDL ships in a
/// forward migration instead of `0001_init`, for the same reason
/// [`REWRITTEN_BY_LATER`] exists: `0001_init` has been applied to deployed
/// databases and `sms-migrate` records migrations by name, so a regenerated
/// `0001_init` would never create them there.
///
/// Every generator statement that names one of these tables is *removed*
/// from the fresh output before the comparison with `0001_init`, and must
/// appear verbatim in the later migration's `up.sql` (or, for the `DROP`s,
/// `down.sql`) - so the forward migration cannot drift from the schema
/// either. One `(directory, tables)` entry per forward migration that adds
/// tables; a new such migration appends an entry. Delete an entry, and its
/// statements, when the migrations are squashed into a regenerated
/// `0001_init`.
const ADDED_BY_LATER: [(&str, &[&str]); 4] = [
    (
        "backends/migrations/postgres/0005_sso_federation",
        &["federated_links"],
    ),
    (
        "backends/migrations/postgres/0006_client_keys",
        &["client_keys"],
    ),
    (
        "backends/migrations/postgres/0007_console_keys",
        &["console_keys"],
    ),
    (
        "backends/migrations/postgres/0008_envelope_nonces",
        &["envelope_nonces"],
    ),
];

/// The forward migration that drops tables an earlier, already-applied
/// migration created (VS-4: the embedded provider's tables), and the tables it
/// drops. The mirror image of [`ADDED_BY_LATER`]: the schema no longer has
/// them, so the generator no longer emits them, while the frozen `0001_init`
/// (and `0002_bootstrap`, `0005_sso_federation`) still create them. Every
/// committed statement that names one of these tables is *removed* from
/// `0001_init` before the comparison, and the migration must drop every table
/// listed (`DROP TABLE <table>;` in its `up.sql`), so the list cannot name a
/// table the migration leaves behind. Delete the entry, and regenerate
/// `0001_init`, when the migrations are squashed.
const DROPPED_BY_LATER: (&str, &[&str]) = (
    "backends/migrations/postgres/0009_drop_openid_provider",
    &[
        "oauth_clients",
        "oauth_signing_keys",
        "client_assertions",
        "user_credentials",
        "sso_transactions",
    ],
);

/// A forward migration that changes the columns of a table `0001_init` already
/// creates (VS-8: `webhook_endpoints` gained `audience` and lost its three
/// secret columns). The statement-level lists above cannot express it: the
/// generator emits the whole `CREATE TABLE` anew, and the applied one differs
/// by exactly these columns. `(directory, table, added, dropped)`.
///
/// Every column definition of `dropped` is removed from the committed
/// `0001_init`, every one of `added` from the generator's output, and the
/// migration's `up.sql` must carry `ALTER TABLE <table> ADD COLUMN <column>`
/// for each added column and `ALTER TABLE <table> DROP COLUMN <column>;` for
/// each dropped one, so the lists cannot name a column the migration leaves
/// alone. Delete the entry, and regenerate `0001_init`, when the migrations are
/// squashed.
const COLUMNS_CHANGED_BY_LATER: (&str, &str, &[&str], &[&str]) = (
    "backends/migrations/postgres/0010_sealed_webhooks",
    "webhook_endpoints",
    &["audience"],
    &["secret", "prev_secret", "secret_rotated_at"],
);

/// Removes its temp directory on drop, on every exit path — success,
/// error-return, or an early `?` — matching the bash version's own
/// `trap 'rm -rf "$out"' EXIT`.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> Result<Self, String> {
        let mut dir = std::env::temp_dir();
        let unique = format!(
            "xtask-migrate-diff-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        dir.push(unique);
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// True when `block` (one blank-line-separated chunk of generator output) is
/// DDL about one of `tables`. Matches the table name as a whole word (or as
/// the prefix of an index/constraint name built from it) in a non-comment
/// line.
fn names_added_table(block: &str, tables: &[&str]) -> bool {
    block
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .any(|line| {
            tables.iter().any(|table| {
                line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .any(|word| word == *table || word.starts_with(&format!("{table}_")))
            })
        })
}

/// Splits `fresh` into the part `0001_init` must equal and the blocks the
/// forward migration must carry. The generator's header comment lists
/// `dbgenerated()` columns as `--   - table.column` lines; those for an
/// added table go too.
fn split_added(fresh: &str, tables: &[&str]) -> (String, Vec<String>) {
    let mut kept = Vec::new();
    let mut moved = Vec::new();
    for block in fresh.split("\n\n") {
        if names_added_table(block, tables) {
            moved.push(block.trim_end_matches('\n').to_owned());
            continue;
        }
        let filtered: Vec<&str> = block
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !(trimmed.starts_with("--   - ")
                    && tables
                        .iter()
                        .any(|table| trimmed.starts_with(&format!("--   - {table}."))))
            })
            .collect();
        kept.push(filtered.join("\n"));
    }
    (kept.join("\n\n"), moved)
}

/// Removes the statements about every table in [`ADDED_BY_LATER`] from
/// `fresh` (the generator's `f`.sql) and checks each is present verbatim in
/// the owning forward migration's own `f`.sql.
fn strip_added_tables(root: &Path, f: &str, fresh: &str) -> Result<String, String> {
    let mut remaining = fresh.to_owned();
    for (dir, tables) in ADDED_BY_LATER {
        let carrier = root.join(dir).join(format!("{f}.sql"));
        let carrier_text =
            fs::read_to_string(&carrier).map_err(|e| format!("{}: {e}", carrier.display()))?;
        let (without_added, moved) = split_added(&remaining, tables);
        for block in &moved {
            if !carrier_text.contains(block.as_str()) {
                return Err(format!(
                    "migrations-current: {} does not contain this statement the generator emits \
                     for a table it carries:\n{block}",
                    carrier.display()
                ));
            }
        }
        remaining = without_added;
    }
    Ok(remaining)
}

/// Removes from the committed `0001_init`'s `f`.sql every statement about a
/// table in [`DROPPED_BY_LATER`], after checking the migration that drops them
/// really does (`DROP TABLE <table>;` in its `up.sql`).
fn strip_dropped_tables(root: &Path, committed: &str) -> Result<String, String> {
    let (dir, tables) = DROPPED_BY_LATER;
    let carrier = root.join(dir).join("up.sql");
    let carrier_text =
        fs::read_to_string(&carrier).map_err(|e| format!("{}: {e}", carrier.display()))?;
    for table in tables {
        let drop = format!("DROP TABLE {table};");
        if !carrier_text.lines().any(|line| line.trim() == drop) {
            return Err(format!(
                "migrations-current: {} does not contain `{drop}`, but DROPPED_BY_LATER says it \
                 drops {table}",
                carrier.display()
            ));
        }
    }
    Ok(split_added(committed, tables).0)
}

/// `sql` without the definition lines of `columns` inside the table's
/// `CREATE TABLE <table> (` block. A column definition is a line whose first word is the column's
/// name; the table's closing `);` ends the block.
fn without_columns(sql: &str, table: &str, columns: &[&str]) -> String {
    let opening = format!("CREATE TABLE {table} (");
    let mut inside = false;
    let mut kept = Vec::new();
    for line in sql.lines() {
        let trimmed = line.trim();
        if trimmed == opening {
            inside = true;
        } else if inside && trimmed == ");" {
            inside = false;
        } else if inside
            && trimmed
                .split_whitespace()
                .next()
                .is_some_and(|word| columns.contains(&word))
        {
            continue;
        }
        kept.push(line);
    }
    kept.join("\n")
}

/// Checks that [`COLUMNS_CHANGED_BY_LATER`]'s migration adds and drops exactly
/// the columns it lists.
fn check_column_carrier(root: &Path) -> Result<(), String> {
    let (dir, table, added, dropped) = COLUMNS_CHANGED_BY_LATER;
    let carrier = root.join(dir).join("up.sql");
    let text = fs::read_to_string(&carrier).map_err(|e| format!("{}: {e}", carrier.display()))?;
    for column in added {
        let wanted = format!("ALTER TABLE {table} ADD COLUMN {column} ");
        if !text.lines().any(|line| line.trim().starts_with(&wanted)) {
            return Err(format!(
                "migrations-current: {} does not contain `{wanted}...`, but \
                 COLUMNS_CHANGED_BY_LATER says it adds {table}.{column}",
                carrier.display()
            ));
        }
    }
    for column in dropped {
        let wanted = format!("ALTER TABLE {table} DROP COLUMN {column};");
        if !text.lines().any(|line| line.trim() == wanted) {
            return Err(format!(
                "migrations-current: {} does not contain `{wanted}`, but \
                 COLUMNS_CHANGED_BY_LATER says it drops {table}.{column}",
                carrier.display()
            ));
        }
    }
    Ok(())
}

/// The committed `0001_init`'s `f`.sql, minus the dropped tables'
/// statements ([`strip_dropped_tables`]).
fn read_committed(root: &Path, f: &str) -> Result<String, String> {
    let path = root.join(COMMITTED_DIR).join(format!("{f}.sql"));
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    strip_dropped_tables(root, &text)
}

/// The statements of a migration file, without its `--` comment lines and
/// blank lines.
///
/// `0001_init` is frozen (see [`REWRITTEN_BY_LATER`]), so the generator
/// changing only what it *says* about a migration must not force an edit to
/// an applied one. cratestack 0.16.0 did exactly that (#1129): it stopped
/// calling a `CHECK` on a table the migration itself creates "blocking", so
/// the WARNING header of `up.sql` and the comment-only `up.pre.sql` scaffold
/// are no longer emitted, while every statement is the same. A statement
/// that changes still fails the comparison; only comment text is ignored.
fn executable_sql(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim_start().is_empty() && !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compares `up.sql` and `down.sql`, committed against regenerated, after
/// taking out everything a later migration owns (added tables, the columns
/// `COLUMNS_CHANGED_BY_LATER` names, statements rewritten afterwards).
fn compare_up_and_down(
    root: &Path,
    regenerated: &Path,
    mismatches: &mut Vec<(&'static str, Vec<String>)>,
) -> Result<(), String> {
    for f in ["up", "down"] {
        let regenerated_path = regenerated.join(format!("{f}.sql"));
        let committed = read_committed(root, f)?;
        let mut fresh = fs::read_to_string(&regenerated_path)
            .map_err(|e| format!("{}: {e}", regenerated_path.display()))?;
        fresh = strip_added_tables(root, f, &fresh)?;
        let mut committed = committed;
        if f == "up" {
            check_column_carrier(root)?;
            let (_, table, added, dropped) = COLUMNS_CHANGED_BY_LATER;
            committed = without_columns(&committed, table, dropped);
            fresh = without_columns(&fresh, table, added);
            let later =
                fs::read_to_string(root.join(LATER_UP)).map_err(|e| format!("{LATER_UP}: {e}"))?;
            for (current, applied) in REWRITTEN_BY_LATER {
                if !later.contains(current) {
                    return Err(format!(
                        "migrations-current: {LATER_UP} no longer contains the statement the \
                         generator emits now:\n  {current}"
                    ));
                }
                fresh = fresh.replace(current, applied);
            }
        }
        if executable_sql(&committed) != executable_sql(&fresh) {
            mismatches.push((f, crate::diff::line_diff(&committed, &fresh)));
        }
    }
    Ok(())
}

pub fn run(root: &Path) -> Result<(), String> {
    let pinned = crate::cratestack_pin::read_pin(root)?;
    let installed = installed_cratestack_version();

    if installed.as_deref() != Some(pinned.as_str()) {
        return Err(format!(
            "migrations-current: installed cratestack CLI ({:?}) does not match the Cargo.toml pin (=\
             {pinned}).\nA mismatched CLI emits DDL the compiled library never produces — see \
             AGENTS.md's 'Regenerating migrations' section.",
            installed.as_deref().unwrap_or("")
        ));
    }

    let scratch = ScratchDir::new()?;

    // `--out-dir` is the *parent* of the backend directory `migrate diff`
    // writes into: it writes "<out>/postgres/<timestamp>_init/{up,up.pre,down}.sql"
    // (up.pre.sql only when a blocking operation is detected), never
    // "<out>/<timestamp>_init/...". A scratch directory outside the
    // repo means `migrate diff`'s own `schema.snapshot.json` side effect —
    // which this repo deliberately never commits — never touches the
    // working tree, so there is nothing to clean up and nothing to confuse
    // a later diff.
    let status = Command::new("cratestack")
        .args(["migrate", "diff", "--schema", SCHEMA, "--out-dir"])
        .arg(&scratch.0)
        .args(["--backend", "postgres", "--name", "init"])
        .current_dir(root)
        .status()
        .map_err(|e| format!("failed to run `cratestack migrate diff`: {e}"))?;

    if !status.success() {
        return Err(format!("`cratestack migrate diff` exited with {status}"));
    }

    let postgres_dir = scratch.0.join("postgres");
    let regenerated = fs::read_dir(&postgres_dir)
        .map_err(|e| format!("{}: {e}", postgres_dir.display()))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with("_init"))
        })
        .ok_or_else(|| {
            format!(
                "migrations-current: cratestack migrate diff produced no *_init directory under {}",
                postgres_dir.display()
            )
        })?;

    // `up.pre.sql` is conditional — `cratestack migrate diff` only
    // scaffolds one when it detects a blocking operation (a `CHECK`/`NOT
    // NULL` addition on a column with no way to prove an existing table
    // has no rows) — so it is compared only when at least one side
    // actually has one, and a missing side reads as empty rather than a
    // hard error: that is precisely the "this migration used to need a
    // backfill and no longer does, or vice versa" drift this check exists
    // to catch, not a broken invocation. `up`/`down` stay hard errors on a
    // missing file, since both are unconditionally emitted every time and
    // a missing one means something else is already broken.
    let up_pre_relevant = regenerated.join("up.pre.sql").is_file()
        || root.join(COMMITTED_DIR).join("up.pre.sql").is_file();

    let mut mismatches = Vec::new();
    compare_up_and_down(root, &regenerated, &mut mismatches)?;
    if up_pre_relevant {
        let committed_path = root.join(COMMITTED_DIR).join("up.pre.sql");
        let regenerated_path = regenerated.join("up.pre.sql");
        let committed = fs::read_to_string(&committed_path).unwrap_or_default();
        let fresh = fs::read_to_string(&regenerated_path).unwrap_or_default();
        if executable_sql(&committed) != executable_sql(&fresh) {
            mismatches.push(("up.pre", crate::diff::line_diff(&committed, &fresh)));
        }
    }

    if mismatches.is_empty() {
        println!(
            "migrations-current: OK — {COMMITTED_DIR} matches `cratestack migrate diff` for the current {SCHEMA}"
        );
        return Ok(());
    }

    let mut msg = String::new();
    for (f, lines) in &mismatches {
        let _ = writeln!(msg, "--- {COMMITTED_DIR}/{f}.sql");
        for line in lines {
            msg.push_str(line);
            msg.push('\n');
        }
    }
    let _ = write!(
        msg,
        "\nmigrations-current: {COMMITTED_DIR}/{{up,up.pre,down}}.sql has drifted from what \
         `cratestack migrate diff` produces from the current {SCHEMA} (diff above).\n\
         Regenerate it (see AGENTS.md's 'Regenerating migrations' section):\n  \
         cratestack migrate diff --schema {SCHEMA} --out-dir backends/migrations --backend postgres --name init\n  \
         # then copy the output over {COMMITTED_DIR}/{{up,up.pre,down}}.sql — up.pre.sql only exists\n  \
         # when the generator scaffolds one; delete the committed copy if it stops emitting one\n  \
         rm -f backends/migrations/postgres/schema.snapshot.json   # this repo does not commit it"
    );
    Err(msg)
}

/// `cratestack --version` output looks like `cratestack 0.7.10`; this reads
/// the second whitespace-separated token, same as the original `awk
/// '{print $2}'`. Returns `None` if the binary is missing or produced no
/// parseable output — mirroring the bash version's own `|| true` fallback
/// to an empty string, which then simply fails the version-equality check
/// below with a readable message instead of a raw "command not found".
fn installed_cratestack_version() -> Option<String> {
    let output = Command::new("cratestack").arg("--version").output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.split_whitespace().nth(1).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_and_blank_lines_are_not_compared() {
        let applied =
            "-- WARNING: blocking\n--   - t.c\n\n-- NOTE: x\nCREATE TABLE t (\n    c TEXT\n);\n";
        let fresh = "-- NOTE: y\nCREATE TABLE t (\n    c TEXT\n);\n";
        assert_eq!(executable_sql(applied), executable_sql(fresh));
        assert_eq!(executable_sql("-- only a comment\n"), executable_sql(""));
    }

    #[test]
    fn each_forward_migration_claims_only_its_own_tables() {
        let fresh = "-- header\n--   - client_keys.id\n--   - federated_links.id\n--   - apps.id\n\n\
                     CREATE TABLE apps (\n    id TEXT\n);\n\n\
                     CREATE TABLE client_keys (\n    id TEXT\n);\n\n\
                     CREATE TABLE federated_links (\n    id TEXT\n);\n\n\
                     CREATE UNIQUE INDEX client_keys_thumbprint_key ON client_keys (thumbprint);";
        let (kept, moved) = split_added(fresh, &["client_keys"]);
        assert_eq!(
            moved.len(),
            2,
            "the table and its index, nothing else: {moved:?}"
        );
        assert!(moved.iter().all(|block| block.contains("client_keys")));
        assert!(kept.contains("CREATE TABLE apps"));
        assert!(
            kept.contains("CREATE TABLE federated_links"),
            "0005's table is not 0006's"
        );
        assert!(!kept.contains("--   - client_keys.id"));
        assert!(kept.contains("--   - federated_links.id"));
    }

    #[test]
    fn a_dropped_table_is_removed_from_the_committed_side_only() {
        let committed = "-- header\n--   - oauth_clients.id\n--   - apps.id\n\n\
                         CREATE TABLE apps (\n    id TEXT\n);\n\n\
                         CREATE TABLE oauth_clients (\n    id TEXT\n);\n\n\
                         CREATE UNIQUE INDEX oauth_clients_client_id_key ON oauth_clients (client_id);\n\n\
                         ALTER TABLE apps ADD CONSTRAINT apps_x CHECK (true);";
        let (kept, moved) = split_added(committed, DROPPED_BY_LATER.1);
        assert_eq!(moved.len(), 2, "the table and its index: {moved:?}");
        assert!(kept.contains("CREATE TABLE apps"));
        assert!(kept.contains("apps_x"));
        assert!(!kept.contains("oauth_clients"));
    }

    #[test]
    fn the_dropping_migration_is_not_also_an_adding_one() {
        let dropped: Vec<&str> = DROPPED_BY_LATER.1.to_vec();
        for (_, tables) in ADDED_BY_LATER {
            assert!(
                tables.iter().all(|table| !dropped.contains(table)),
                "a table cannot be both added by a later migration and dropped by one"
            );
        }
    }

    #[test]
    fn every_forward_migration_directory_is_listed_once() {
        let dirs: Vec<&str> = ADDED_BY_LATER.iter().map(|(dir, _)| *dir).collect();
        let mut unique = dirs.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(dirs.len(), unique.len());
        assert!(ADDED_BY_LATER.iter().all(|(_, tables)| !tables.is_empty()));
    }

    #[test]
    fn a_column_change_is_removed_from_the_side_that_lacks_it_only() {
        let applied = "CREATE TABLE t (\n    id TEXT NOT NULL,\n    secret TEXT NOT NULL,\n    \
                       prev_secret TEXT,\n    PRIMARY KEY (id)\n);\n";
        let fresh = "CREATE TABLE t (\n    id TEXT NOT NULL,\n    audience TEXT NOT NULL,\n    \
                     PRIMARY KEY (id)\n);\n";
        let a = without_columns(applied, "t", &["secret", "prev_secret"]);
        let b = without_columns(fresh, "t", &["audience"]);
        assert_eq!(executable_sql(&a), executable_sql(&b));
        // Another table's column of the same name is not touched.
        let other = "CREATE TABLE u (\n    secret TEXT\n);\n";
        assert!(without_columns(other, "t", &["secret"]).contains("secret"));
        // A column the list does not name still differs.
        let drifted = "CREATE TABLE t (\n    id TEXT NOT NULL,\n    audience TEXT NOT NULL,\n    \
                       extra TEXT,\n    PRIMARY KEY (id)\n);\n";
        let c = without_columns(drifted, "t", &["audience"]);
        assert_ne!(executable_sql(&a), executable_sql(&c));
    }

    #[test]
    fn the_column_migration_is_not_one_of_the_table_migrations() {
        let dir = COLUMNS_CHANGED_BY_LATER.0;
        assert_ne!(dir, DROPPED_BY_LATER.0);
        assert!(ADDED_BY_LATER.iter().all(|(added, _)| *added != dir));
    }

    #[test]
    fn a_changed_statement_still_differs() {
        let a = "-- same\nCREATE TABLE t (\n    c TEXT\n);\n";
        let b = "-- same\nCREATE TABLE t (\n    c TEXT NOT NULL\n);\n";
        assert_ne!(executable_sql(a), executable_sql(b));
        // A statement commented out is a removed statement, not a comment change.
        assert_ne!(
            executable_sql(a),
            executable_sql("-- same\n-- CREATE TABLE t (\n")
        );
    }
}
