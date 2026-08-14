//! `pg_dump`/`pg_restore` themselves — real binary-format dump/restore
//! tools nobody reimplements in application code, kept as external
//! processes on purpose (same posture `app/sms-migrate`'s own module doc
//! takes toward `psql \i`: replace the *orchestration*, never the
//! database engine's own tooling). The runtime image still ships
//! `postgres:16-alpine` for exactly these two binaries — see this crate's
//! own `Dockerfile`.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

pub fn dump_custom_format(database_url: &str, out_path: &Path) -> Result<()> {
    let status = Command::new("pg_dump")
        .arg(database_url)
        .arg("--format=custom")
        .arg("--file")
        .arg(out_path)
        .status()
        .context("running pg_dump")?;
    if !status.success() {
        bail!("pg_dump exited with {status}");
    }
    Ok(())
}

pub fn restore_custom_format(database_url: &str, dump_path: &Path) -> Result<()> {
    let status = Command::new("pg_restore")
        .arg("--dbname")
        .arg(database_url)
        .args(["--clean", "--if-exists", "--no-owner", "--no-privileges"])
        .arg(dump_path)
        .status()
        .context("running pg_restore")?;
    if !status.success() {
        bail!("pg_restore exited with {status}");
    }
    Ok(())
}

/// The last whitespace-separated token of `pg_dump --version`'s own
/// output (`pg_dump (PostgreSQL) 16.4` -> `16.4`) — same extraction the
/// old `backup.sh` did with `awk '{print $NF}'`.
pub fn pg_dump_version() -> Result<String> {
    let output = Command::new("pg_dump")
        .arg("--version")
        .output()
        .context("running pg_dump --version")?;
    if !output.status.success() {
        bail!("pg_dump --version exited with {}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .last()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("pg_dump --version produced no parseable output: {text:?}"))
}
