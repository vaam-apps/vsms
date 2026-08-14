//! `restore` — the reusable restore primitive both an incident and
//! `restore-drill` build on. Direct port of the old `deploy/restore.sh`;
//! restores INTO an existing, reachable database, never creates one.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::manifest::{pepper_fingerprint, Manifest};
use crate::util::ScratchDir;
use crate::{pgtools, rclone};

pub enum RestoreSource {
    /// The newest `vsms-*.dump` in `rclone_remote`, by lexical (=
    /// chronological) sort — mirrors the old script's own `rclone lsf |
    /// sort | tail -n1`.
    Latest,
    /// A specific, already-known filename in `rclone_remote` — either
    /// typed by an operator, or (from `restore-drill`) the exact name
    /// [`crate::backup::run`] just produced.
    Named(String),
    /// A dump already on disk — no `rclone` involved at all.
    Local(PathBuf),
}

pub struct RestoreConfig {
    pub database_url: String,
    pub rclone_remote: Option<String>,
    /// Compared against the manifest's own stored fingerprint. `None`
    /// skips the check with a warning, matching the old script's
    /// "`SMS_HASH_PEPPER` not set locally" branch.
    pub hash_pepper: Option<String>,
    /// The one guard standing between this subcommand and dropping every
    /// object in whatever `database_url` points at. No default, and
    /// deliberately an explicit boolean the caller has to construct from
    /// an env var read — see `main.rs`'s own `RESTORE_CONFIRM_OVERWRITE`
    /// handling for why this stays an env var, not a CLI flag (the same
    /// reasoning the old script's own comment gives: no risk of an
    /// argument-order mistake between a dump name and a confirmation
    /// during a real outage).
    pub confirmed: bool,
}

pub fn run(config: &RestoreConfig, source: RestoreSource) -> Result<()> {
    let scratch = ScratchDir::new("vsms-restore")?;

    let (dump_path, manifest_path): (PathBuf, PathBuf) = match source {
        RestoreSource::Local(path) => {
            let manifest_path = path.with_extension("").with_extension("manifest.json");
            (path, manifest_path)
        }
        RestoreSource::Latest | RestoreSource::Named(_) => {
            let remote = config
                .rclone_remote
                .as_deref()
                .context("BACKUP_RCLONE_REMOTE must be set to pull a named or latest backup")?;
            let dump_name = match source {
                RestoreSource::Named(name) => name,
                RestoreSource::Latest => {
                    let names = rclone::list_sorted(remote, "vsms-*.dump")?;
                    names
                        .into_iter()
                        .last()
                        .with_context(|| format!("no backups found in {remote}"))?
                }
                RestoreSource::Local(_) => unreachable!("handled by the outer match arm"),
            };
            let manifest_name = format!("{}.manifest.json", strip_dump_suffix(&dump_name));
            let dump_path = scratch.0.join(&dump_name);
            let manifest_path = scratch.0.join(&manifest_name);

            println!("vsms-backup: pulling {dump_name} from {remote}");
            rclone::copyto_remote(&format!("{remote}/{dump_name}"), &dump_path)?;
            if !rclone::try_copyto_remote(&format!("{remote}/{manifest_name}"), &manifest_path) {
                eprintln!(
                    "vsms-backup: no manifest found for {dump_name} — skipping the pepper \
                     fingerprint check"
                );
            }
            (dump_path, manifest_path)
        }
    };

    check_pepper(&manifest_path, config.hash_pepper.as_deref());

    if !config.confirmed {
        bail!(
            "vsms-backup: refusing to overwrite {} without confirmation. This drops and \
             recreates every object in the target database. Re-run with \
             RESTORE_CONFIRM_OVERWRITE=yes once you have checked DATABASE_URL.",
            crate::db::redact(&config.database_url),
        );
    }

    println!(
        "vsms-backup: restoring into {}",
        crate::db::redact(&config.database_url)
    );
    pgtools::restore_custom_format(&config.database_url, &dump_path)?;
    println!("vsms-backup: restore done");
    Ok(())
}

/// `"vsms-20260814T120000Z.dump"` -> `"vsms-20260814T120000Z"`.
fn strip_dump_suffix(dump_name: &str) -> &str {
    dump_name.strip_suffix(".dump").unwrap_or(dump_name)
}

/// Warns, never blocks — restoring under a deliberately different pepper
/// is a legitimate DR choice in some scenarios (see
/// `docs/runbooks/backup-restore.md`'s own "the pepper is part of the
/// recoverable state" section), but it must never be a *silent* one.
fn check_pepper(manifest_path: &Path, hash_pepper: Option<&str>) {
    let Ok(contents) = std::fs::read_to_string(manifest_path) else {
        eprintln!(
            "vsms-backup: no manifest available — cannot check whether this backup's pepper \
             matches the current SMS_HASH_PEPPER. Proceeding blind; see \
             docs/runbooks/backup-restore.md."
        );
        return;
    };
    let manifest: Manifest = match serde_json::from_str(&contents) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!(
                "vsms-backup: manifest at {} did not parse ({error}) — skipping the pepper \
                 fingerprint check",
                manifest_path.display()
            );
            return;
        }
    };

    let Some(pepper) = hash_pepper else {
        eprintln!(
            "vsms-backup: SMS_HASH_PEPPER not set locally — skipping the pepper fingerprint \
             check (this backup's stored fingerprint: {}).",
            manifest.pepper_fingerprint_sha256
        );
        return;
    };

    let current_fp = pepper_fingerprint(pepper);
    if current_fp == manifest.pepper_fingerprint_sha256 {
        println!(
            "vsms-backup: pepper fingerprint matches — restored hashes stay comparable under \
             the current SMS_HASH_PEPPER."
        );
    } else {
        eprintln!(
            "vsms-backup: WARNING — SMS_HASH_PEPPER does not match the pepper this backup was \
             taken under."
        );
        eprintln!(
            "vsms-backup: msisdnHash/bodyHash in the restored rows will not match anything \
             hashed under the current pepper — opt-out and dedupe checks silently stop matching \
             old rows. See crates/sms-api/src/pepper.rs and docs/runbooks/backup-restore.md \
             before proceeding."
        );
    }
}
