//! Thin wrappers over the real `rclone` binary — the object-storage layer
//! stays exactly what `docs/runbooks/backup-restore.md` chose it for
//! (provider-agnostic: S3, B2, GCS, Azure Blob, MinIO, or a bare local
//! path all work behind the same three calls below), unreimplemented,
//! unreplaced. This module is process orchestration, not a client library
//! — the same shape `.xtask/src/migrations_current.rs` already uses to
//! shell out to the real `cratestack` CLI in the main workspace.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

fn run(mut command: Command, what: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("running rclone {what}"))?;
    if !status.success() {
        bail!("rclone {what} exited with {status}");
    }
    Ok(())
}

pub fn copyto(source: &Path, dest_remote_path: &str) -> Result<()> {
    let mut command = Command::new("rclone");
    command.arg("copyto").arg(source).arg(dest_remote_path);
    run(
        command,
        &format!("copyto {} {dest_remote_path}", source.display()),
    )
}

pub fn copyto_remote(source_remote_path: &str, dest: &Path) -> Result<()> {
    let mut command = Command::new("rclone");
    command.arg("copyto").arg(source_remote_path).arg(dest);
    run(
        command,
        &format!("copyto {source_remote_path} {}", dest.display()),
    )
}

/// `true` on success — callers that treat "no manifest present" as a
/// soft, warn-and-continue case (matching the old `restore.sh`'s own
/// `2>/dev/null || echo ...`) check this instead of propagating the error.
pub fn try_copyto_remote(source_remote_path: &str, dest: &Path) -> bool {
    Command::new("rclone")
        .arg("copyto")
        .arg(source_remote_path)
        .arg(dest)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Lists filenames directly under `remote` matching `include` (an rclone
/// glob, e.g. `"vsms-*.dump"`), lexically sorted ascending — timestamped
/// names (`vsms-<RFC3339-ish>.dump`) sort chronologically this way, so the
/// last entry is the newest, matching the old script's own `sort | tail
/// -n1`.
pub fn list_sorted(remote: &str, include: &str) -> Result<Vec<String>> {
    let output = Command::new("rclone")
        .args(["lsf", remote, "--include", include])
        .output()
        .with_context(|| format!("running rclone lsf {remote} --include {include}"))?;
    if !output.status.success() {
        bail!(
            "rclone lsf {remote} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect();
    names.sort();
    Ok(names)
}

/// Best-effort, matching the old scripts' own `|| true` on both prune
/// calls — a failed prune is not a failed backup.
pub fn delete_older_than(remote: &str, min_age_days: u32, include: &str) {
    let status = Command::new("rclone")
        .args([
            "delete",
            remote,
            "--min-age",
            &format!("{min_age_days}d"),
            "--include",
            include,
        ])
        .status();
    if !matches!(status, Ok(status) if status.success()) {
        eprintln!(
            "vsms-backup: rclone delete --min-age {min_age_days}d --include {include} against \
             {remote} did not succeed — continuing (pruning is best-effort, not part of the \
             backup's own success condition)"
        );
    }
}
