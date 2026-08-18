#![doc = include_str!("reap_outbox.md")]

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
// `cratestack::sqlx` the module, not individual items — every call site
// below spells the raw calls out fully-qualified (`sqlx::query(...)`,
// `sqlx::query_as(...)`), matching `backends/apps/sms-migrate/src/main.rs`'s own
// convention for the same reason: `cargo xtask no-raw-sqlx`'s pattern
// matches the literal substring `sqlx::query`/`sqlx::query_as`/`sqlx::raw_sql`,
// so the exception has to be visible at the call site, not hidden behind a
// braced multi-item `use` the check can't see through.
use cratestack::CratestackContext;
use cratestack::sqlx;
use sms_api::schema::{Cratestack, Job};
use tracing::{debug, info, warn};

use crate::jobs::JobHandler;

/// §7.5's own retention: a delivered row survives this long before this
/// job deletes it.
const DELIVERED_RETENTION: Duration = Duration::hours(24);

/// #42's own threshold, verbatim: "alert on `attempts > 5`". Applied only
/// to still-undelivered rows — see the module doc.
const POISON_ATTEMPTS_THRESHOLD: i64 = 5;

/// Bounds one run's `DELETE` — same reasoning as `expire_stale::BATCH`: a
/// backlog beyond this is picked up by this job's own next hourly run
/// rather than one invocation trying to clear an unbounded table in a
/// single statement.
const DELETE_BATCH: i64 = 1000;

/// Bounds how many poison rows one run logs. The row stays in the table
/// either way — a smaller, representative sample of `warn!` lines is as
/// actionable as thousands of identical ones, and anything left over is
/// alerted on again next run.
const ALERT_BATCH: i64 = 200;

/// The `reap_outbox` [`JobHandler`] — see the module doc for what "reap"
/// means here and why poison rows are alarmed on rather than deleted.
pub struct ReapOutbox;

impl ReapOutbox {
    /// The testable core, the same seam `ExpireStale::run_at` uses and for
    /// the same reason: `occurred_at`/`delivered_at` are stamped by the
    /// framework itself (`enqueue_event_outbox`/`drain_event_outbox`, both
    /// in `cratestack-sqlx`, both using `NOW()`/`Utc::now()` internally)
    /// with no delegate seam this crate could backdate through, R1
    /// exception or not — nothing legitimately issues an
    /// `UPDATE ... SET occurred_at = ...`. Live tests instead pass a `now`
    /// far enough past a genuinely-just-now row that the 24h retention has
    /// "elapsed" relative to that virtual clock, exactly the trick
    /// `ExpireStale`'s own `uncertain`-grace test already uses for
    /// `Message.updatedAt`.
    pub async fn run_at(&self, db: &Cratestack, now: DateTime<Utc>) -> Result<(), String> {
        let poisoned = alert_poison_rows(db)
            .await
            .map_err(|error| format!("scanning the event outbox for poison rows: {error}"))?;
        // #70: the one writer of `sms_event_outbox_poison_rows` — see
        // `sms_metrics`'s own module doc for why this gauge doesn't need
        // the absent-vs-zero treatment the two per-role gauges do (this
        // job is claimed via CAS, not held via an advisory lock, so "0"
        // from a process that has simply never won a claim is already
        // correct, not a false all-clear).
        sms_metrics::EVENT_OUTBOX_POISON_ROWS.set(i64::try_from(poisoned).unwrap_or(i64::MAX));
        if poisoned > 0 {
            warn!(
                poisoned,
                "poison event outbox rows found this run — see the per-row logs above"
            );
        }

        let cutoff = now - DELIVERED_RETENTION;
        let reaped = reap_delivered(db, cutoff)
            .await
            .map_err(|error| format!("reaping delivered event outbox rows: {error}"))?;

        if reaped > 0 {
            info!(reaped, "reaped delivered event outbox rows past retention");
        } else {
            debug!("no delivered event outbox rows past retention to reap");
        }

        Ok(())
    }
}

#[async_trait]
impl JobHandler for ReapOutbox {
    fn kind(&self) -> &'static str {
        "reap_outbox"
    }

    async fn run(
        &self,
        db: &Cratestack,
        _sys: &CratestackContext,
        _job: &Job,
    ) -> Result<(), String> {
        self.run_at(db, Utc::now()).await
    }
}

/// `true` if `error` is Postgres's `42P01 undefined_table` — the outbox
/// table has never been created. See the module doc for why this job
/// treats that as "nothing to reap yet" rather than a fault, instead of
/// duplicating the framework's own table DDL here.
fn is_undefined_table(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .as_deref()
        == Some("42P01")
}

/// Log every still-undelivered row past [`POISON_ATTEMPTS_THRESHOLD`], up
/// to [`ALERT_BATCH`] of them, and return how many were found. Never
/// deletes, never touches `attempts` — see the module doc for why a poison
/// row is surfaced, not removed.
///
/// `pub` for the same reason `drain::oldest_undelivered_age` is: live tests
/// assert the exact count directly against a real Postgres, rather than
/// scraping a `tracing` log line out of this function's own `warn!` calls
/// — the same convention that module's doc already states explicitly.
pub async fn alert_poison_rows(db: &Cratestack) -> Result<usize, sqlx::Error> {
    type PoisonRow = (String, String, String, DateTime<Utc>, i64, Option<String>);

    let rows: Vec<PoisonRow> = match sqlx::query_as(
        "SELECT event_id::text, model, operation, occurred_at, attempts, last_error \
         FROM cratestack_event_outbox \
         WHERE delivered_at IS NULL AND attempts > $1 \
         ORDER BY occurred_at ASC \
         LIMIT $2",
    )
    .bind(POISON_ATTEMPTS_THRESHOLD)
    .bind(ALERT_BATCH)
    .fetch_all(db.pool())
    .await
    {
        Ok(rows) => rows,
        Err(error) if is_undefined_table(&error) => return Ok(0),
        Err(error) => return Err(error),
    };

    for (event_id, model, operation, occurred_at, attempts, last_error) in &rows {
        warn!(
            event_id,
            model,
            operation,
            %occurred_at,
            attempts,
            last_error = last_error.as_deref().unwrap_or("<none recorded>"),
            "poison event outbox row: attempts exceeds threshold with no successful delivery; \
             left in place for diagnosis, not deleted"
        );
    }

    Ok(rows.len())
}

/// Delete delivered rows older than `cutoff`, up to [`DELETE_BATCH`] of
/// them, and return how many were deleted. Postgres has no
/// `DELETE ... LIMIT`; the `IN (SELECT ...)` form is the standard batching
/// idiom for it.
///
/// `pub` for the same reason [`alert_poison_rows`] is.
pub async fn reap_delivered(db: &Cratestack, cutoff: DateTime<Utc>) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM cratestack_event_outbox \
         WHERE event_id IN ( \
             SELECT event_id FROM cratestack_event_outbox \
             WHERE delivered_at IS NOT NULL AND delivered_at < $1 \
             ORDER BY delivered_at ASC \
             LIMIT $2 \
         )",
    )
    .bind(cutoff)
    .bind(DELETE_BATCH)
    .execute(db.pool())
    .await;

    match result {
        Ok(done) => Ok(done.rows_affected()),
        Err(error) if is_undefined_table(&error) => Ok(0),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{DELIVERED_RETENTION, POISON_ATTEMPTS_THRESHOLD, ReapOutbox};
    use crate::jobs::JobHandler;

    #[test]
    fn kind_matches_the_scheduler_and_design_docs_naming() {
        assert_eq!(ReapOutbox.kind(), "reap_outbox");
    }

    #[test]
    fn retention_and_threshold_match_the_design_doc_and_issue_text() {
        // §7.5: "Delete delivered cratestack_event_outbox rows >24h."
        assert_eq!(DELIVERED_RETENTION, chrono::Duration::hours(24));
        // #42's own issue text: "alert on attempts > 5".
        assert_eq!(POISON_ATTEMPTS_THRESHOLD, 5);
    }
}
