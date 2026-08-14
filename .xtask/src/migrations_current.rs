//! `0001_init` matches what `cratestack migrate diff` actually produces
//! from the current `schema/schema.cstack`.
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

const SCHEMA: &str = "schema/schema.cstack";
const COMMITTED_DIR: &str = "schema/migrations/postgres/0001_init";

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
    // writes into: it writes "<out>/postgres/<timestamp>_init/{up,down}.sql",
    // never "<out>/<timestamp>_init/...". A scratch directory outside the
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

    let mut mismatches = Vec::new();
    for f in ["up", "down"] {
        let committed_path = root.join(COMMITTED_DIR).join(format!("{f}.sql"));
        let regenerated_path = regenerated.join(format!("{f}.sql"));
        let committed = fs::read_to_string(&committed_path)
            .map_err(|e| format!("{}: {e}", committed_path.display()))?;
        let fresh = fs::read_to_string(&regenerated_path)
            .map_err(|e| format!("{}: {e}", regenerated_path.display()))?;
        if committed != fresh {
            mismatches.push((f, crate::diff::line_diff(&committed, &fresh)));
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
        "\nmigrations-current: {COMMITTED_DIR}/{{up,down}}.sql has drifted from what \
         `cratestack migrate diff` produces from the current {SCHEMA} (diff above).\n\
         Regenerate it (see AGENTS.md's 'Regenerating migrations' section):\n  \
         cratestack migrate diff --schema {SCHEMA} --out-dir schema/migrations --backend postgres --name init\n  \
         # then copy the output over {COMMITTED_DIR}/{{up,down}}.sql\n  \
         rm -f schema/migrations/postgres/schema.snapshot.json   # this repo does not commit it"
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
