#![doc = include_str!("drill.md")]

use anyhow::{bail, Result};
use rand::Rng;

use crate::backup::{self, BackupConfig};
use crate::db;
use crate::restore::{self, RestoreConfig, RestoreSource};
use crate::util::ScratchDir;

pub struct DrillConfig {
    pub database_url: String,
    pub hash_pepper: Option<String>,
    /// `None` means "use a throwaway local directory for this drill run
    /// only" — matches the old script's own behaviour when
    /// `BACKUP_RCLONE_REMOTE` is unset, so the drill has zero external
    /// dependencies by default. `Some` exercises a real rclone remote
    /// (S3, B2, ...) end to end instead.
    pub rclone_remote: Option<String>,
}

pub fn run(config: DrillConfig, confirmed: bool) -> Result<()> {
    if !confirmed {
        bail!(
            "vsms-backup: refusing to run restore-drill without the confirmation flag. This \
             destroys every object in DATABASE_URL and restores from a fresh backup. Point it \
             at a scratch database only. Re-run with \
             --yes-i-understand-this-destroys-the-target-database."
        );
    }

    let hash_pepper = config
        .hash_pepper
        .unwrap_or_else(random_pepper_for_a_drill_with_no_configured_pepper);

    // A throwaway local directory this drill owns, dropped at the end of
    // this function's scope — only constructed when the caller supplied
    // no real remote. `rclone` treats a bare local path as a valid remote
    // spec on its own, so `backup`/`restore` need no special-casing for
    // "the remote happens to be a directory."
    let own_remote_dir;
    let rclone_remote = match &config.rclone_remote {
        Some(remote) => remote.clone(),
        None => {
            own_remote_dir = ScratchDir::new("vsms-backup-drill-remote")?;
            let remote = own_remote_dir.0.join("remote");
            std::fs::create_dir_all(&remote)?;
            println!(
                "vsms-backup: BACKUP_RCLONE_REMOTE not set — using a throwaway local directory \
                 ({}). Set BACKUP_RCLONE_REMOTE to also exercise a real object-storage remote.",
                remote.display()
            );
            remote.to_string_lossy().into_owned()
        }
    };

    let mut client = db::connect(&config.database_url)?;

    println!("== 1/6: seeding a recognisable marker row ==");
    let marker_id = format!("drill-{}", chrono::Utc::now().timestamp());
    db::seed_marker(&mut client, &marker_id)?;
    println!("marker: {marker_id}");

    println!("== 2/6: recording exact row counts before backup ==");
    let before = db::row_counts(&mut client)?;
    println!("before: {before:?}");
    drop(client);

    println!("== 3/6: taking a backup (the same code path the scheduled service runs) ==");
    let dump_name = backup::run(&BackupConfig {
        database_url: config.database_url.clone(),
        hash_pepper: hash_pepper.clone(),
        rclone_remote: rclone_remote.clone(),
        retention_days: 30,
    })?;
    println!("backup: {dump_name}");

    println!("== 4/6: destroying every object in the target database ==");
    let mut client = db::connect(&config.database_url)?;
    db::destroy_public_schema(&mut client)?;
    let remaining = db::count_public_tables(&mut client)?;
    println!("tables remaining in public after DROP: {remaining}");
    if remaining != 0 {
        bail!("expected 0 tables after DROP SCHEMA, got {remaining}");
    }
    drop(client);

    println!("== 5/6: restoring from the backup ==");
    restore::run(
        &RestoreConfig {
            database_url: config.database_url.clone(),
            rclone_remote: Some(rclone_remote),
            hash_pepper: Some(hash_pepper),
            confirmed: true,
        },
        RestoreSource::Named(dump_name),
    )?;

    println!("== 6/6: verifying ==");
    let mut client = db::connect(&config.database_url)?;
    let after = db::row_counts(&mut client)?;
    println!("after:  {after:?}");
    let marker_after = db::read_marker_note(&mut client, &marker_id)?;

    let mut failed = false;
    if before != after {
        eprintln!("RESTORE DRILL FAILED: row counts differ before vs after.");
        eprintln!("  before: {before:?}");
        eprintln!("  after:  {after:?}");
        failed = true;
    }
    if marker_after.as_deref() != Some("restore-drill-proof") {
        eprintln!(
            "RESTORE DRILL FAILED: marker row '{marker_id}' missing or wrong after restore \
             (got {marker_after:?})."
        );
        failed = true;
    }

    if failed {
        bail!("restore drill failed — see the RESTORE DRILL FAILED lines above");
    }

    println!(
        "RESTORE DRILL PASSED: exact row counts match, and marker row '{marker_id}' survived \
         backup + destroy + restore intact."
    );
    Ok(())
}

/// `SMS_HASH_PEPPER:=$(openssl rand -base64 48)`'s replacement — 48
/// random bytes, base64-encoded (standard alphabet, matching `openssl
/// base64`'s own default), for a drill run with no real pepper supplied.
/// The scheduled `backup` subcommand always gets a real one from
/// `SMS_HASH_PEPPER` (required, no default) and never exercises this path.
fn random_pepper_for_a_drill_with_no_configured_pepper() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 48];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
