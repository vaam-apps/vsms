//! `backup` — takes a single `pg_dump` of `DATABASE_URL` in custom
//! format, writes the manifest next to it, pushes both to the configured
//! `rclone` remote, and prunes anything past the retention window. Direct
//! port of the old `deploy/backup.sh`; see that file's own git history
//! for the shell version this replaces and `docs/runbooks/backup-restore.md`
//! for the full "why pg_dump, not WAL archiving" reasoning, unchanged by
//! this rewrite.

use anyhow::{Context, Result};
use chrono::Utc;

use crate::manifest::{pepper_fingerprint, Manifest};
use crate::util::ScratchDir;
use crate::{db, pgtools, rclone};

pub struct BackupConfig {
    pub database_url: String,
    pub hash_pepper: String,
    pub rclone_remote: String,
    pub retention_days: u32,
}

/// Returns the `.dump` filename this run produced (`vsms-<ts>.dump`) —
/// `restore-drill` uses it directly rather than re-discovering it via
/// `rclone lsf` immediately afterward, since the actual upload still goes
/// through real `rclone copyto` calls either way; `restore --latest`'s own
/// discovery-by-listing path is exercised separately, by that subcommand.
pub fn run(config: &BackupConfig) -> Result<String> {
    let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let workdir = ScratchDir::new("vsms-backup")?;

    let dump_name = format!("vsms-{ts}.dump");
    let manifest_name = format!("vsms-{ts}.manifest.json");
    let dump_path = workdir.0.join(&dump_name);
    let manifest_path = workdir.0.join(&manifest_name);

    println!(
        "vsms-backup: dumping {} -> {dump_name}",
        db::redact(&config.database_url)
    );
    pgtools::dump_custom_format(&config.database_url, &dump_path)?;

    let mut client = db::connect(&config.database_url)?;
    let schema_migrations_applied = db::applied_migrations(&mut client)?;
    drop(client);

    let manifest = Manifest {
        taken_at: ts,
        pg_dump_format: "custom".to_owned(),
        postgres_version: pgtools::pg_dump_version()?,
        pepper_fingerprint_sha256: pepper_fingerprint(&config.hash_pepper),
        schema_migrations_applied,
    };
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).context("serialising the manifest")?,
    )
    .with_context(|| format!("writing {}", manifest_path.display()))?;

    println!("vsms-backup: uploading to {}", config.rclone_remote);
    rclone::copyto(&dump_path, &format!("{}/{dump_name}", config.rclone_remote))?;
    rclone::copyto(
        &manifest_path,
        &format!("{}/{manifest_name}", config.rclone_remote),
    )?;

    println!(
        "vsms-backup: pruning backups older than {}d in {}",
        config.retention_days, config.rclone_remote
    );
    rclone::delete_older_than(&config.rclone_remote, config.retention_days, "vsms-*.dump");
    rclone::delete_older_than(
        &config.rclone_remote,
        config.retention_days,
        "vsms-*.manifest.json",
    );

    println!("vsms-backup: done — {dump_name}");
    Ok(dump_name)
}
