#![doc = include_str!("schedule.md")]
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use chrono::Utc;
use cron::Schedule;
use std::str::FromStr;

use crate::backup::{self, BackupConfig};

pub struct ScheduleConfig {
    pub backup: BackupConfig,
    pub cron_expression: String,
    pub run_on_start: bool,
    /// Touched (mtime bumped) after every *successful* backup —
    /// [`check_health`] is what reads it back. See that function's own
    /// doc for why this exists at all: `docker inspect`'s health status
    /// should reflect whether backups are actually landing, not merely
    /// that this process hasn't crashed.
    pub health_file: std::path::PathBuf,
}

/// How long a single wait-for-the-next-tick iteration sleeps before
/// re-checking the shutdown flag — bounds how long a `SIGTERM` can take
/// to actually be noticed, not how often a backup runs.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// `check_health`'s own fallback when the cron expression's period can't
/// be derived empirically (see that function's doc) — two days, safely
/// above `BACKUP_CRON_SCHEDULE`'s own documented default cadence (daily).
const FALLBACK_PERIOD: Duration = Duration::from_secs(48 * 60 * 60);

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
///
/// `pub(crate)`, not private: [`check_health`] parses `BACKUP_CRON_SCHEDULE`
/// a second time (the healthcheck is a genuinely separate process
/// invocation from `schedule`'s own — see `Command::Healthcheck`'s doc in
/// `main.rs`), and reusing this exact function is what keeps the two
/// interpretations of that env var from silently drifting apart.
pub(crate) fn normalize_to_seconds_first(expression: &str) -> String {
    let field_count = expression.split_whitespace().count();
    if field_count == 5 {
        format!("0 {expression}")
    } else {
        expression.to_owned()
    }
}

fn parse_schedule(cron_expression: &str) -> Result<Schedule> {
    let normalized = normalize_to_seconds_first(cron_expression);
    Schedule::from_str(&normalized).with_context(|| {
        format!(
            "BACKUP_CRON_SCHEDULE={cron_expression:?} is not a valid cron expression (normalized \
             to {normalized:?} — see normalize_to_seconds_first's own doc)"
        )
    })
}

/// Bumps `path`'s mtime (creating it if absent) — used for both
/// [`ScheduleConfig::health_file`] (the one signal [`check_health`] has
/// that a backup actually succeeded, since a stale rclone remote's own
/// listing is a network call this local, frequent exec-form healthcheck
/// should not have to make) and, since review round 1 item 15, the
/// start-marker file [`start_marker_path`] derives next to it. Failure
/// to write either is logged, not propagated: a healthcheck-plumbing
/// write failure must never fail an otherwise-successful backup, or stop
/// `schedule` from starting at all.
fn touch_health_file(path: &Path) {
    if let Err(error) = std::fs::write(path, Utc::now().to_rfc3339()) {
        eprintln!(
            "vsms-backup: failed to touch health file {} (backup itself still succeeded): \
             {error:#}",
            path.display()
        );
    }
}

/// Where `run` records the moment this process actually started, next
/// to `health_file` — review round 1, item 15: without this, a
/// `BACKUP_RUN_ON_START=false` container (or one still running a
/// genuinely slow first backup) has no health file at all yet, and
/// [`check_health`] would report unhealthy for up to a full schedule
/// period after every single restart, indistinguishable from a
/// deployment where backups have silently stopped working. The start
/// marker gives [`check_health`] a second, independent "this is
/// expected, not broken" signal for exactly that window.
fn start_marker_path(health_file: &Path) -> std::path::PathBuf {
    let mut path = health_file.as_os_str().to_owned();
    path.push(".started");
    std::path::PathBuf::from(path)
}

pub fn run(config: ScheduleConfig) -> Result<()> {
    let schedule = parse_schedule(&config.cron_expression)?;
    println!("vsms-backup: schedule = {}", config.cron_expression);

    // Review round 1, item 15: written unconditionally, before anything
    // else — including before `BACKUP_RUN_ON_START`'s own branch below,
    // which may not touch `health_file` at all for a long time (or ever,
    // if every scheduled backup keeps failing). See
    // `start_marker_path`'s own doc for why this exists.
    touch_health_file(&start_marker_path(&config.health_file));

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
        match backup::run(&config.backup) {
            Ok(_) => touch_health_file(&config.health_file),
            Err(error) => eprintln!(
                "vsms-backup: initial backup failed — will retry on the next scheduled run \
                 ({}): {error:#}",
                config.cron_expression
            ),
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

        match backup::run(&config.backup) {
            Ok(_) => touch_health_file(&config.health_file),
            Err(error) => eprintln!(
                "vsms-backup: scheduled backup failed — will retry on the next scheduled run: \
                 {error:#}"
            ),
        }
    }

    println!("vsms-backup: shutdown requested — exiting");
    Ok(())
}

/// Derives the schedule's own approximate period empirically — the gap
/// between the next two upcoming occurrences from now — rather than
/// reading it off the cron expression's syntax directly, which `cron`'s
/// own `Schedule` has no general concept of for an arbitrary (possibly
/// irregular) expression. Stable for every fixed-cadence expression this
/// repo documents (`0 3 * * *` and friends); falls back to
/// [`FALLBACK_PERIOD`] if fewer than two occurrences can be computed at
/// all (not reachable for a schedule `parse_schedule` already accepted in
/// practice, but a defensive floor rather than a panic).
fn approximate_period(schedule: &Schedule) -> Duration {
    let mut upcoming = schedule.upcoming(Utc);
    let (Some(first), Some(second)) = (upcoming.next(), upcoming.next()) else {
        return FALLBACK_PERIOD;
    };
    (second - first).to_std().unwrap_or(FALLBACK_PERIOD)
}

/// `Command::Healthcheck`'s body (`main.rs`) — a real signal that backups
/// are actually landing, not just "is the `schedule` process alive."
/// Compares [`touch_health_file`]'s own mtime against `now - 2 *
/// <the schedule's own period>` (see [`approximate_period`]): one missed
/// backup alone doesn't fail this check (a transient `rclone`/`pg_dump`
/// hiccup is already retried on the very next tick, per `run`'s own
/// `Err` arm above), but two in a row past the schedule's own cadence is
/// a real, actionable signal.
///
/// A missing health file falls back to [`start_marker_path`] (review
/// round 1, item 15) rather than reporting unhealthy outright: found
/// live, `BACKUP_RUN_ON_START=false` (or a container still running a
/// genuinely slow first backup) left no health file at all for up to a
/// full schedule period after every restart, indistinguishable from a
/// deployment whose backups have actually stopped working — the exact
/// false-positive the Dockerfile's own `--start-period` cannot fully
/// paper over on its own, since `--start-period` only covers the first
/// container start, not every subsequent restart of a long-lived
/// service. The start marker is written unconditionally the instant
/// `run` starts, so it gives this check the same 2x-period grace window
/// a successful backup gets, without waiting for one to actually land.
/// Only if *neither* file exists (this container isn't running
/// `schedule` at all, or something deleted both) does this report
/// unhealthy immediately.
pub fn check_health(health_file: &Path, cron_expression: &str) -> Result<()> {
    let schedule = parse_schedule(cron_expression)?;
    let period = approximate_period(&schedule);
    let max_age = period * 2;

    match std::fs::metadata(health_file) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .context("reading the health file's own mtime")?;
            let age = SystemTime::now()
                .duration_since(modified)
                .unwrap_or(Duration::ZERO);
            if age > max_age {
                anyhow::bail!(
                    "last successful backup is {age:?} old — older than 2x the schedule's own \
                     ~{period:?} period ({max_age:?}, from \
                     BACKUP_CRON_SCHEDULE={cron_expression:?})"
                );
            }
            println!("vsms-backup: healthy — last successful backup {age:?} ago (max {max_age:?})");
            Ok(())
        }
        Err(_) => check_health_via_start_marker(health_file, cron_expression, max_age, period),
    }
}

/// The no-successful-backup-yet half of [`check_health`] — split out
/// purely to keep that function under `clippy::too_many_lines`.
fn check_health_via_start_marker(
    health_file: &Path,
    cron_expression: &str,
    max_age: Duration,
    period: Duration,
) -> Result<()> {
    let marker = start_marker_path(health_file);
    let marker_metadata = std::fs::metadata(&marker).with_context(|| {
        format!(
            "no successful backup recorded yet at {} and no start marker at {} either — this \
             container may not be running `vsms-backup schedule` at all",
            health_file.display(),
            marker.display()
        )
    })?;
    let started = marker_metadata
        .modified()
        .context("reading the start marker's own mtime")?;
    let age = SystemTime::now()
        .duration_since(started)
        .unwrap_or(Duration::ZERO);

    if age > max_age {
        anyhow::bail!(
            "no successful backup recorded at {} within 2x the schedule's own ~{period:?} \
             period ({max_age:?}) since this process started {age:?} ago (from \
             BACKUP_CRON_SCHEDULE={cron_expression:?})",
            health_file.display()
        );
    }
    println!(
        "vsms-backup: healthy — no backup has succeeded yet, but this process only started \
         {age:?} ago (within the {max_age:?} grace period)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approximate_period_of_a_daily_schedule_is_about_a_day() {
        let schedule = parse_schedule("0 3 * * *").expect("a valid 5-field cron expression");
        let period = approximate_period(&schedule);
        assert_eq!(period, Duration::from_secs(24 * 60 * 60));
    }

    #[test]
    fn a_missing_health_file_and_no_start_marker_is_unhealthy() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let missing = dir.path().join("never-written");
        let error = check_health(&missing, "0 3 * * *").expect_err("neither file was ever written");
        assert!(
            format!("{error:#}").contains("no start marker at"),
            "unexpected error: {error:#}"
        );
    }

    /// Review round 1, item 15: a container that hasn't produced a
    /// successful backup yet (`BACKUP_RUN_ON_START=false`, or still
    /// running a genuinely slow first backup) must not be reported
    /// unhealthy just because `health_file` doesn't exist yet, as long as
    /// `run` started recently enough — the start marker is what proves
    /// "recently enough" here instead of just asserting it.
    #[test]
    fn a_missing_health_file_with_a_fresh_start_marker_is_healthy() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let health_file = dir.path().join("never-written");
        touch_health_file(&start_marker_path(&health_file));
        check_health(&health_file, "0 3 * * *")
            .expect("a process that only just started must not be reported unhealthy yet");
    }

    /// The other half of item 15: once the start-marker's own grace
    /// window has elapsed with still no successful backup, this must
    /// become a real, actionable failure again, not stay silently
    /// healthy forever just because the process once started.
    #[test]
    fn a_missing_health_file_with_a_stale_start_marker_is_unhealthy() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let health_file = dir.path().join("never-written");
        let marker = start_marker_path(&health_file);
        touch_health_file(&marker);
        // Same two-days-and-one-second-past-the-2x-daily-max-age
        // backdating `a_stale_health_file_is_unhealthy` below uses.
        let stale = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60 + 1);
        let file = std::fs::File::open(&marker).expect("reopening the fixture");
        file.set_modified(stale).expect("backdating the mtime");
        drop(file);
        let error = check_health(&health_file, "0 3 * * *")
            .expect_err("the start marker's own grace period has long since elapsed");
        assert!(
            format!("{error:#}").contains("no successful backup recorded at"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn a_fresh_health_file_is_healthy() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let path = dir.path().join("fresh");
        std::fs::write(&path, "just now").expect("writing the fixture");
        check_health(&path, "0 3 * * *").expect("a file touched moments ago is within 2x a day");
    }

    #[test]
    fn a_stale_health_file_is_unhealthy() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let path = dir.path().join("stale");
        std::fs::write(&path, "long ago").expect("writing the fixture");
        // Two days and one second in the past — past the 2x-a-day max age
        // a "0 3 * * *" schedule computes.
        let stale = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60 + 1);
        let file = std::fs::File::open(&path).expect("reopening the fixture");
        file.set_modified(stale).expect("backdating the mtime");
        drop(file);
        let error = check_health(&path, "0 3 * * *").expect_err("this file is stale");
        assert!(
            format!("{error:#}").contains("older than 2x the schedule's own"),
            "unexpected error: {error:#}"
        );
    }
}
