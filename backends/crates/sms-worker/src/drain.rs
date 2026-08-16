#![doc = include_str!("drain.md")]

use std::time::Duration as StdDuration;

use chrono::Duration;
use cratestack::CoolError;
use cratestack::sqlx::query_scalar;
use sms_api::schema::Cratestack;
use tracing::{debug, error, warn};

use crate::WorkerContext;

/// How often this role calls `.events().drain()`. §7.1/§39: every 5s.
const TICK_INTERVAL: StdDuration = StdDuration::from_secs(5);

/// Oldest-undelivered age past which a tick logs at `warn` instead of
/// `debug`. A generous multiple of [`TICK_INTERVAL`] — a single slow tick
/// or one transient handler failure shouldn't page anyone; an outbox row
/// that's been sitting for multiple minutes with nothing draining it is
/// the actual "writes have gone quiet and nobody noticed" scenario #39
/// exists to surface.
const STALLED_THRESHOLD: Duration = Duration::minutes(2);

/// Never returns on its own, matching [`crate::run`]'s contract for every
/// other role.
pub async fn run(ctx: WorkerContext, _worker: &str) {
    let mut interval = tokio::time::interval(TICK_INTERVAL);
    loop {
        interval.tick().await;
        tick(&ctx.db).await;
    }
}

/// One poll iteration — `pub` for the same reason `dispatch::tick` and
/// `jobs::tick`/`scheduler::tick` are: live tests drive exactly one
/// iteration deterministically rather than racing [`run`]'s own timer.
// `age.num_seconds() as f64` below only loses precision past 2^53 seconds
// (~285 million years) — irrelevant for an outbox staleness gauge, and the
// same reasoning `oldest_undelivered_age`'s own `cast_possible_truncation`
// allow already documents for the same value's other cast.
#[allow(clippy::cast_precision_loss)]
pub async fn tick(db: &Cratestack) {
    match db.events().drain().await {
        Ok(delivered) => debug!(delivered, "drained the event outbox"),
        Err(error) => error!(%error, "draining the event outbox failed; retrying next tick"),
    }

    match oldest_undelivered_age(db).await {
        Ok(Some(age)) if age >= STALLED_THRESHOLD => {
            warn!(
                age_secs = age.num_seconds(),
                threshold_secs = STALLED_THRESHOLD.num_seconds(),
                "oldest undelivered webhook outbox event exceeds the stalled threshold"
            );
            // #70: set on every branch that actually knows the age — see
            // `sms_metrics`'s own doc for why only the process holding
            // `drain`'s lease ever reaches this line, and why that's what
            // makes the metric's absence, cluster-wide, mean "drain is
            // unheld" rather than "nothing is stalled."
            sms_metrics::WEBHOOK_OUTBOX_OLDEST_UNDELIVERED_AGE_SECONDS
                .set(age.num_seconds() as f64);
        }
        Ok(Some(age)) => {
            debug!(age_secs = age.num_seconds(), "oldest undelivered event age");
            sms_metrics::WEBHOOK_OUTBOX_OLDEST_UNDELIVERED_AGE_SECONDS
                .set(age.num_seconds() as f64);
        }
        Ok(None) => {
            debug!("event outbox has no undelivered rows");
            sms_metrics::WEBHOOK_OUTBOX_OLDEST_UNDELIVERED_AGE_SECONDS.set(0.0);
        }
        Err(error) => {
            error!(%error, "reading oldest-undelivered event age failed");
        }
    }
}

/// How long the oldest still-undelivered `cratestack_event_outbox` row has
/// been waiting, or `None` if nothing is undelivered. See this module's
/// own doc for why this is a named R1 exception rather than a delegate
/// call — there is no delegate for a table that isn't a schema model.
///
/// `pub` for the same reason `tick` is: `drain_live_postgres.rs` asserts
/// this directly against a real Postgres rather than trying to scrape a
/// `tracing` log line out of `tick`'s own `warn!`/`debug!` calls.
pub async fn oldest_undelivered_age(db: &Cratestack) -> Result<Option<Duration>, CoolError> {
    // `ensure_event_outbox_table` doesn't run here — `db.events().drain()`
    // already ran it unconditionally, immediately before this is ever
    // called (see `tick`), so the table is guaranteed to exist by the time
    // this query runs.
    // `EXTRACT(EPOCH FROM ...)` returns `NUMERIC`, not `FLOAT8` — found
    // live, not assumed: sqlx's `Option<f64>` decode rejected it outright
    // ("mismatched types ... NUMERIC"). The explicit `::float8` cast is
    // what makes the column type match what's actually bound below.
    let age_seconds: Option<f64> = query_scalar(
        "SELECT EXTRACT(EPOCH FROM (NOW() - MIN(occurred_at)))::float8 \
         FROM cratestack_event_outbox WHERE delivered_at IS NULL",
    )
    .fetch_one(db.pool())
    .await
    .map_err(|error| {
        CoolError::Internal(format!("reading oldest undelivered event age: {error}"))
    })?;

    // Sub-second precision doesn't matter for a staleness alert — this age
    // only ever feeds a `>= STALLED_THRESHOLD` comparison and a log field.
    #[allow(clippy::cast_possible_truncation)]
    let millis = age_seconds.map(|seconds| (seconds * 1000.0) as i64);
    Ok(millis.map(Duration::milliseconds))
}
