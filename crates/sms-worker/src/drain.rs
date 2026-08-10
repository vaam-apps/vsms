//! `Role::Drain`'s real body — #39. `db.events().drain()` on an interval,
//! per §7.1's own one-line description of this role. Singleton (§7.1):
//! concurrent drains are safe — the unique index on `(endpoint_id,
//! aggregate_id, event_type)` catches a double-insert — but every
//! duplicate drain is wasted work and a wasted index probe, and §8.3 says
//! so explicitly.
//!
//! # What this role actually adds, given #38's subscribers already run
//! synchronously
//!
//! See `sms_api::webhooks`'s own module doc for the full resolution of
//! "if subscribers already insert `WebhookAttempt` rows synchronously
//! (#38), what does this role drain?" — required reading before touching
//! this file. The short version: every `@@emit`-annotated mutation already
//! triggers an automatic post-commit drain of its own process's runtime,
//! so as long as `sms_api::webhooks::register_subscribers` has been
//! called on this process's `Cratestack` instance — which
//! `app/sms-worker`'s `main` does exactly once, unconditionally, before
//! spawning any role task, not gated on `drain` being one of `--roles` —
//! most events are already turned into `WebhookAttempt` rows inline, by
//! whatever wrote them.
//!
//! What this role adds is the one thing no writer's own post-commit drain
//! gives you: a **write-independent** retry trigger for a row whose
//! handler failed on an earlier attempt (a transient error creating the
//! `WebhookAttempt` row, say — `drain_event_outbox` records `attempts`/
//! `last_error` and leaves `delivered_at IS NULL` on any handler `Err`).
//! Nothing else in this codebase calls `.events().drain()` on a timer —
//! without this role, such a row sits undelivered until the next
//! unrelated write happens to touch an emitting model, which "writes go
//! quiet" (#39's own framing, and the framework's own §8.2: "no
//! background drain worker exists") can leave open indefinitely.
//!
//! # Alerting on oldest-undelivered age, not just on errors
//!
//! #39's own acceptance line is explicit that an error count alone isn't
//! enough — a stalled outbox with zero errors (nothing has failed, no
//! `WebhookAttempt` was ever attempted because no drain ever ran) is
//! exactly as silent as one full of retries. [`oldest_undelivered_age`]
//! answers "how long has the oldest still-undelivered event been
//! waiting", logged every tick at `warn` once it crosses
//! [`STALLED_THRESHOLD`] — a log line an ops dashboard can alert on, the
//! same convention `lease.rs`'s own "alert on this" framing and R2's
//! "alert on any non-zero SM001 rate" use elsewhere in this codebase; no
//! metrics/alerting pipeline exists yet in this workspace to wire a
//! counter into instead.
//!
//! **R1 exception, the fifth one.** `cratestack_event_outbox` is the
//! framework's own internal bookkeeping table (created lazily by
//! `ensure_event_outbox_table`, not one of `schema.cstack`'s models) — no
//! delegate exists to read it, and none of the four already-named
//! exceptions (migrations, `pg_advisory_lock`, `LISTEN`/`NOTIFY`,
//! `/readyz`'s bare `SELECT 1`) cover it either. Reading
//! `MIN(occurred_at) WHERE delivered_at IS NULL` here is a fifth, for the
//! same reason `/readyz`'s exception exists: there is no row-level policy
//! to bypass (the table isn't part of this schema), no audit trail to
//! skip (a `SELECT` isn't a mutation), and no `@version`/soft-delete
//! concern (it isn't a model at all). `ci/assert-no-raw-sqlx.sh` and
//! `CONTRIBUTING.md`'s own R1 exceptions table both name this file.

use std::time::Duration as StdDuration;

use chrono::Duration;
use cratestack::sqlx::query_scalar;
use cratestack::CoolError;
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
        }
        Ok(Some(age)) => {
            debug!(age_secs = age.num_seconds(), "oldest undelivered event age");
        }
        Ok(None) => debug!("event outbox has no undelivered rows"),
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
