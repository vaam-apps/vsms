//! `reap_outbox` — #42. The job kind named in §7.5's own table: "Delete
//! delivered `cratestack_event_outbox` rows >24h; alarm on high-`attempts`
//! rows." §8.2 of the design doc states the framework side of the problem
//! outright: "`attempts`/`last_error` are recorded but never read: no
//! retry cap, no backoff, no dead-letter. A permanently failing handler
//! retries that row forever and the table grows without bound." Confirmed
//! directly against the vendored source
//! (`cratestack-sqlx-0.7.10/src/descriptor.rs`'s `drain_event_outbox`), not
//! assumed from that prose: every failed delivery attempt only ever
//! `UPDATE`s `attempts = attempts + 1, last_error = $2` and leaves
//! `delivered_at` `NULL` — there is no comparison against any cap anywhere
//! in that function, and no code path anywhere in `cratestack-sqlx` deletes
//! a row of this table at all, ever. This job is the entire mechanism;
//! nothing upstream partially covers it.
//!
//! # Reap means delete delivered rows, not poison ones — deliberately
//!
//! §7.5's own table already answers "reap or quarantine": delete
//! **delivered** rows past retention, and separately **alarm** on
//! high-`attempts` (still-undelivered) rows — never delete those. That
//! split is the actual design decision here, not incidental phrasing:
//!
//! - A **delivered** row (`delivered_at IS NOT NULL`) has already done its
//!   job — the event reached a `WebhookAttempt` row, or (today) had no
//!   subscriber registered for it at all. Keeping it past a day is pure
//!   bloat: nothing in this codebase ever reads a delivered outbox row
//!   again.
//! - A **poison** row (`delivered_at IS NULL`, `attempts` past the
//!   threshold) is the opposite: it is live evidence of a bug — a
//!   subscriber that keeps failing on the same event, forever, per §8.2's
//!   "short-circuits on the first failing handler" behaviour. Deleting it
//!   would erase the only record that the bug happened *and* silently drop
//!   the event it was trying to redeliver — a customer-visible data loss
//!   with no trace it ever occurred, which is a strictly worse outcome
//!   than "the table is a bit bigger than it should be." So a poison row
//!   is left exactly where it is — `attempts`/`last_error`/`occurred_at`
//!   untouched — and this job instead makes it loud: a `warn!` per row,
//!   every run, carrying `model`/`operation`/`last_error` so an operator
//!   can diagnose the actual subscriber bug rather than have this job
//!   quietly hide the symptom.
//!
//!   "Quarantine" in the sense of moving the row to a separate table was
//!   considered and rejected: there is no schema model backing this table
//!   in the first place (the whole reason reading it needs an R1 exception
//!   — see below), so a quarantine table would just be a *second*
//!   hand-rolled, delegate-less, policy-less table for no benefit over
//!   leaving the row in place and alerting on it loudly.
//!
//! # What actually constitutes a poison row
//!
//! Not "any row with `attempts > 5`" on its own — that would also catch a
//! row still legitimately mid-retry. `drain::tick` (#39) polls every 5s
//! with no backoff of its own, so a row can rack up several `attempts`
//! within its first couple of minutes purely from that polling cadence
//! colliding with a slow-to-recover subscriber, not because anything is
//! actually stuck. The `attempts > 5` threshold from #42's own issue text
//! is applied only to rows that are **still undelivered**
//! (`delivered_at IS NULL`) — a delivered row's `attempts` count is just
//! "how many tries it took," not a signal of anything wrong, and this job
//! never alerts on it.
//!
//! # R1 exception, the sixth one
//!
//! Same reasoning as `drain.rs`'s own fifth exception, restated because
//! this is a different file: `cratestack_event_outbox` is the framework's
//! own lazily-created bookkeeping table (`ensure_event_outbox_table`), not
//! one of `schema.cstack`'s models — no delegate exists to read or write
//! it, so there is no row-level policy to bypass, no audit trail to skip,
//! no `@version`/soft-delete concern. `ci/assert-no-raw-sqlx.sh` and
//! `CONTRIBUTING.md`'s own exceptions table both name this file.
//!
//! Unlike `drain.rs`, this job cannot lean on `db.events().drain()` having
//! already run `ensure_event_outbox_table` immediately beforehand — this
//! job never calls `.drain()` at all, and nothing guarantees it runs after
//! some other write already has (a fresh deployment could have this job's
//! own schedule fire before the first event is ever emitted). Rather than
//! duplicate that table's DDL here — a second, silently-drifting copy of a
//! definition this crate doesn't own — both queries below treat Postgres's
//! `42P01` ("`undefined_table`") as "nothing to reap yet" and return success:
//! correct, because a table that was never created has, by construction,
//! no delivered rows to reap and no poison rows to alarm on.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
// `cratestack::sqlx` the module, not individual items — every call site
// below spells the raw calls out fully-qualified (`sqlx::query(...)`,
// `sqlx::query_as(...)`), matching `app/sms-migrate/src/main.rs`'s own
// convention for the same reason: `ci/assert-no-raw-sqlx.sh`'s grep looks
// for the literal substring `sqlx::query`/`sqlx::query_as`/`sqlx::raw_sql`,
// so the exception has to be visible at the call site, not hidden behind a
// braced multi-item `use` the grep can't see through.
use cratestack::sqlx;
use cratestack::CoolContext;
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

    async fn run(&self, db: &Cratestack, _sys: &CoolContext, _job: &Job) -> Result<(), String> {
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
    use super::{ReapOutbox, DELIVERED_RETENTION, POISON_ATTEMPTS_THRESHOLD};
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
