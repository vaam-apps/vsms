//! `vsms-backup` — the Rust replacement for
//! `deploy/{backup,restore,restore-drill,backup-entrypoint}.sh`. #69's
//! own framing is still the design: "backups that have never been
//! restored are not backups" — this binary's `restore-drill` subcommand
//! is that proof, not just `backup`/`restore` themselves.
//!
//! # Why `pg_dump`/`pg_restore`/`rclone` stay external processes
//!
//! This is not the `app/sms-migrate` shape (a hand-rolled SQL runner
//! replacing `psql \i`) — `pg_dump`'s custom format and `pg_restore`'s
//! selective/parallel replay are a real, non-trivial binary protocol with
//! no reason to reimplement, and `rclone` is the one piece of this
//! mechanism that is deliberately provider-agnostic (S3, B2, GCS, Azure
//! Blob, MinIO, or a bare local path, all behind the same three calls —
//! see `docs/runbooks/backup-restore.md`'s own "the bucket is the
//! operator's choice" section). What this binary *does* replace: every
//! `psql -Atc "..."` ad-hoc query (now typed functions over a real
//! `postgres::Client`, `db.rs`), the manifest's own hand-built JSON
//! (`manifest.rs`, `serde`-typed), and — the part that used to need a
//! second daemon — Alpine's busybox `crond` plus a hand-written
//! `/etc/crontabs/root` line (`schedule.rs`, an in-process cron-expression
//! scheduler with its own graceful-shutdown handling).
//!
//! # Why this is a separate Cargo workspace, not `app/sms-backup`
//!
//! See `Cargo.toml`'s own header — same reasoning `examples/rust` and
//! `sdks/rust/vsms-sdk-rust` already establish for staying out of the
//! root workspace, applied here because this crate has nothing to do
//! with the schema/framework the root workspace's `include_server_schema!`
//! memory budget and MSRV are actually about, and depends on nothing from
//! `crates/`/`app/` at all.

mod backup;
mod db;
mod drill;
mod manifest;
mod pgtools;
mod rclone;
mod restore;
mod schedule;
mod util;

use std::path::PathBuf;

use anyhow::Result;
use clap::{ArgGroup, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "vsms-backup",
    version,
    about = "pg_dump/pg_restore + rclone backup, restore, and the drill that proves it works"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Take one backup right now: `pg_dump --format=custom`, a manifest,
    /// upload both to `BACKUP_RCLONE_REMOTE`, prune anything past
    /// `BACKUP_RETENTION_DAYS`.
    Backup {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        /// #134: real secret material — must match whatever `sms-gateway`
        /// is running with right now, or this backup's own manifest
        /// records a fingerprint restoring under a different pepper will
        /// visibly disagree with (see `restore`'s own pepper check).
        #[arg(long, env = "SMS_HASH_PEPPER")]
        hash_pepper: String,

        #[arg(long, env = "BACKUP_RCLONE_REMOTE")]
        rclone_remote: String,

        #[arg(long, env = "BACKUP_RETENTION_DAYS", default_value_t = 30)]
        retention_days: u32,
    },

    /// Restore a backup into `DATABASE_URL`. Refuses to run without
    /// `RESTORE_CONFIRM_OVERWRITE=yes` — a bare env var, not a CLI flag,
    /// so an argument-order mistake between a dump name and a
    /// confirmation can't happen during a real outage.
    #[command(group(
        ArgGroup::new("source")
            .args(["latest", "dump_name", "local"])
            .required(true)
    ))]
    Restore {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        #[arg(long, env = "BACKUP_RCLONE_REMOTE")]
        rclone_remote: Option<String>,

        #[arg(long, env = "SMS_HASH_PEPPER")]
        hash_pepper: Option<String>,

        #[arg(long, env = "RESTORE_CONFIRM_OVERWRITE")]
        confirm_overwrite: Option<String>,

        /// The newest `vsms-*.dump` in `--rclone-remote`.
        #[arg(long)]
        latest: bool,

        /// A specific, already-known filename in `--rclone-remote`.
        #[arg(long)]
        dump_name: Option<String>,

        /// A dump already on disk — no `rclone` involved.
        #[arg(long)]
        local: Option<PathBuf>,
    },

    /// #69's own gate. Seeds a marker row, records exact row counts,
    /// backs up, destroys every object in `DATABASE_URL`, restores, and
    /// diffs before vs after. **Never point this at a database with data
    /// you care about** — requires the confirmation flag for exactly
    /// that reason, and there is no default target.
    RestoreDrill {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        #[arg(long, env = "SMS_HASH_PEPPER")]
        hash_pepper: Option<String>,

        /// Omit to use a throwaway local directory (zero external
        /// dependencies); set it to also exercise a real object-storage
        /// remote's upload/download path, not just `pg_dump`/`pg_restore`.
        #[arg(long, env = "BACKUP_RCLONE_REMOTE")]
        rclone_remote: Option<String>,

        #[arg(long)]
        yes_i_understand_this_destroys_the_target_database: bool,
    },

    /// This container's own entrypoint (`deploy/backup.Dockerfile`'s
    /// `ENTRYPOINT`) — runs an initial backup (unless
    /// `BACKUP_RUN_ON_START=false`), then blocks, backing up again on
    /// every `BACKUP_CRON_SCHEDULE` tick, forever, until `SIGTERM`/`SIGINT`.
    Schedule {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        #[arg(long, env = "SMS_HASH_PEPPER")]
        hash_pepper: String,

        #[arg(long, env = "BACKUP_RCLONE_REMOTE")]
        rclone_remote: String,

        #[arg(long, env = "BACKUP_RETENTION_DAYS", default_value_t = 30)]
        retention_days: u32,

        #[arg(long, env = "BACKUP_CRON_SCHEDULE", default_value = "0 3 * * *")]
        cron_schedule: String,

        #[arg(long, env = "BACKUP_RUN_ON_START", default_value_t = true)]
        run_on_start: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Backup {
            database_url,
            hash_pepper,
            rclone_remote,
            retention_days,
        } => {
            backup::run(&backup::BackupConfig {
                database_url,
                hash_pepper,
                rclone_remote,
                retention_days,
            })?;
            Ok(())
        }

        Command::Restore {
            database_url,
            rclone_remote,
            hash_pepper,
            confirm_overwrite,
            latest,
            dump_name,
            local,
        } => {
            let source = if let Some(path) = local {
                restore::RestoreSource::Local(path)
            } else if let Some(name) = dump_name {
                restore::RestoreSource::Named(name)
            } else {
                debug_assert!(latest, "clap's ArgGroup guarantees one of the three is set");
                restore::RestoreSource::Latest
            };
            restore::run(
                &restore::RestoreConfig {
                    database_url,
                    rclone_remote,
                    hash_pepper,
                    confirmed: confirm_overwrite.as_deref() == Some("yes"),
                },
                source,
            )
        }

        Command::RestoreDrill {
            database_url,
            hash_pepper,
            rclone_remote,
            yes_i_understand_this_destroys_the_target_database,
        } => drill::run(
            drill::DrillConfig {
                database_url,
                hash_pepper,
                rclone_remote,
            },
            yes_i_understand_this_destroys_the_target_database,
        ),

        Command::Schedule {
            database_url,
            hash_pepper,
            rclone_remote,
            retention_days,
            cron_schedule,
            run_on_start,
        } => schedule::run(schedule::ScheduleConfig {
            backup: backup::BackupConfig {
                database_url,
                hash_pepper,
                rclone_remote,
                retention_days,
            },
            cron_expression: cron_schedule,
            run_on_start,
        }),
    }
}
