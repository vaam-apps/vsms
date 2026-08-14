//! `schedule` — this container's own entrypoint when run unattended
//! (`deploy/backup.Dockerfile`'s `ENTRYPOINT`). Replaces the old
//! `deploy/backup-entrypoint.sh` + Alpine's busybox `crond` entirely, not
//! just the shell wrapper around it: `cron` parses `BACKUP_CRON_SCHEDULE`
//! and computes the next occurrence in-process, so there is no
//! `/etc/crontabs/root` file to write, no `/proc/1/fd/{1,2}` redirection
//! trick to route crond's own mail-by-default output back to this
//! container's stdout/stderr (this binary, being PID 1 itself, already
//! *is* that stdout/stderr — nothing to redirect), and no second process
//! for this container to be liveness-checked through.
//!
//! One correctness property the old shell entrypoint got for free by
//! `exec`ing into `crond` and never had to think about: **this container
//! is PID 1 inside its own PID namespace**, and the Linux kernel exempts
//! PID 1 from a signal's *default* disposition unless the process has
//! explicitly installed a handler for it — so an unhandled `SIGTERM`
//! here would be silently ignored, not "terminate like every other
//! process," and `docker stop`/`docker compose down` would hang for the
//! full stop-grace-period before falling back to `SIGKILL`, every time.
//! `signal_hook` below is what makes this exit promptly instead.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use cron::Schedule;
use std::str::FromStr;

use crate::backup::{self, BackupConfig};

pub struct ScheduleConfig {
    pub backup: BackupConfig,
    pub cron_expression: String,
    pub run_on_start: bool,
}

/// How long a single wait-for-the-next-tick iteration sleeps before
/// re-checking the shutdown flag — bounds how long a `SIGTERM` can take
/// to actually be noticed, not how often a backup runs.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// `BACKUP_CRON_SCHEDULE`'s default (`0 3 * * *`, and every value
/// `deploy/docker-compose.yml`/this repo's own docs already document) is
/// standard 5-field POSIX crontab syntax (minute hour day month weekday).
/// The `cron` crate this binary uses instead requires a *seconds* field
/// first (6 or 7 fields total, seconds-then-POSIX) — a real, easy-to-miss
/// mismatch checked directly against that crate's own parser, not
/// assumed. Rather than force every existing `BACKUP_CRON_SCHEDULE` value
/// to be rewritten, a plain 5-field expression is detected by field count
/// and given a leading `0` (seconds) automatically; a 6-or-7-field
/// expression (already seconds-first) passes through unchanged, so an
/// operator who *does* want second-level precision can still have it.
fn normalize_to_seconds_first(expression: &str) -> String {
    let field_count = expression.split_whitespace().count();
    if field_count == 5 {
        format!("0 {expression}")
    } else {
        expression.to_owned()
    }
}

pub fn run(config: ScheduleConfig) -> Result<()> {
    let normalized = normalize_to_seconds_first(&config.cron_expression);
    let schedule = Schedule::from_str(&normalized).with_context(|| {
        format!(
            "BACKUP_CRON_SCHEDULE={:?} is not a valid cron expression (normalized to {normalized:?} \
             — see normalize_to_seconds_first's own doc)",
            config.cron_expression
        )
    })?;
    println!("vsms-backup: schedule = {}", config.cron_expression);

    let shutdown = Arc::new(AtomicBool::new(false));
    // SIGTERM (`docker stop`'s own signal) and SIGINT (Ctrl-C, for a
    // developer running this by hand) both request the same clean exit.
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown))
        .context("registering the SIGTERM handler")?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown))
        .context("registering the SIGINT handler")?;

    if config.run_on_start {
        println!(
            "vsms-backup: BACKUP_RUN_ON_START=true — running an initial backup before the \
             first scheduled tick"
        );
        if let Err(error) = backup::run(&config.backup) {
            eprintln!(
                "vsms-backup: initial backup failed — will retry on the next scheduled run \
                 ({}): {error:#}",
                config.cron_expression
            );
        }
    }

    while !shutdown.load(Ordering::Relaxed) {
        let Some(next) = schedule.upcoming(Utc).next() else {
            // A syntactically valid expression with no future occurrence
            // at all (e.g. Feb 30th) — not reachable with `cron`'s own
            // supported field ranges in practice, but a schedule that can
            // never fire again is a configuration bug, not something to
            // spin-loop on.
            anyhow::bail!(
                "BACKUP_CRON_SCHEDULE={:?} has no upcoming occurrence",
                config.cron_expression
            );
        };
        let now = Utc::now();
        let remaining = (next - now).to_std().unwrap_or(Duration::ZERO);
        println!("vsms-backup: next backup at {next} (in {remaining:?})");

        let mut slept = Duration::ZERO;
        while slept < remaining {
            if shutdown.load(Ordering::Relaxed) {
                println!("vsms-backup: shutdown requested — exiting");
                return Ok(());
            }
            let step = POLL_INTERVAL.min(remaining - slept);
            std::thread::sleep(step);
            slept += step;
        }

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        if let Err(error) = backup::run(&config.backup) {
            eprintln!(
                "vsms-backup: scheduled backup failed — will retry on the next scheduled run: \
                 {error:#}"
            );
        }
    }

    println!("vsms-backup: shutdown requested — exiting");
    Ok(())
}
