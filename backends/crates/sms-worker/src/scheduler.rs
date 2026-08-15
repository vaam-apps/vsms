#![doc = include_str!("scheduler.md")]

use std::collections::HashMap;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::errors::UNIQUE_VIOLATION;
use sms_api::schema::{Cratestack, CreateJobInput, job};
use tracing::{error, warn};

use crate::WorkerContext;

/// How often this role checks whether anything is due. Finer than the
/// coarsest registered cadence so a 1-minute job doesn't slip toward
/// 1-minute-plus-jitter — cheap for a singleton role with no per-tick
/// external cost.
const TICK_INTERVAL: StdDuration = StdDuration::from_secs(5);

/// One recurring `kind`, per §7.5's table. `pub` (and its fields with it)
/// so live tests can build a throwaway spec with a short `cadence`,
/// rather than waiting out the real one-minute `expire_stale` cadence to
/// prove dedupe/cadence behaviour.
pub struct RecurringJobSpec {
    /// `Job.kind` — must match a [`crate::jobs::JobHandler::kind`] for the
    /// enqueued rows to ever actually run.
    pub kind: &'static str,
    /// Minimum gap between two enqueues of this `kind`.
    pub cadence: Duration,
    /// `Job.priority`, carried onto every row this spec enqueues.
    pub priority: i64,
    /// `Job.maxAttempts`, carried onto every row this spec enqueues.
    pub max_attempts: i64,
}

/// This milestone's registry — see the module doc for scope. `pub` for
/// the same reason [`RecurringJobSpec`] is.
#[must_use]
pub fn schedule() -> Vec<RecurringJobSpec> {
    vec![
        RecurringJobSpec {
            kind: "expire_stale",
            cadence: Duration::minutes(1),
            priority: 500,
            max_attempts: 3,
        },
        RecurringJobSpec {
            // §7.5's own cadence for this kind.
            kind: "reap_outbox",
            cadence: Duration::hours(1),
            priority: 500,
            max_attempts: 3,
        },
        RecurringJobSpec {
            // §7.5's own cadence for this kind — "daily". Lower priority
            // than the other two: purging data a caller can no longer act
            // on is real but never urgent the way an unreclaimed lease or a
            // poison outbox row is.
            kind: "purge_retention",
            cadence: Duration::days(1),
            priority: 100,
            max_attempts: 3,
        },
        RecurringJobSpec {
            // §7.5's own cadence for this kind — "daily". Same priority
            // band as purge_retention: a compliance artifact, not
            // operationally urgent the way an unreclaimed lease or a
            // poison outbox row is.
            kind: "anchor_audit",
            cadence: Duration::days(1),
            priority: 100,
            max_attempts: 3,
        },
        RecurringJobSpec {
            // #64: not one of §7.5's own named kinds — see
            // crate::jobs::grey_route_watch's own module doc. Daily is
            // enough for both halves it checks: the divergence check
            // recomputes a fresh 7-day rolling window every run regardless
            // of how often it's called, and handset-validation staleness
            // moves on the order of days/weeks, not minutes. Same priority
            // band as the other daily compliance/observability jobs.
            kind: "grey_route_watch",
            cadence: Duration::days(1),
            priority: 100,
            max_attempts: 3,
        },
    ]
}

fn sys(worker: &str) -> CoolContext {
    Principal {
        sub: format!("sms-worker:scheduler:{worker}"),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// Never returns on its own, matching [`crate::run`]'s contract.
pub async fn run(ctx: WorkerContext, worker: &str) {
    let sys = sys(worker);
    let specs = schedule();
    let mut last_enqueued = seed_last_enqueued(&ctx.db, &sys, &specs).await;

    let mut interval = tokio::time::interval(TICK_INTERVAL);
    loop {
        interval.tick().await;
        tick(&ctx.db, &sys, &specs, &mut last_enqueued).await;
    }
}

/// The most recent `Job` row per registered `kind`, regardless of its
/// state — a fresh deploy with no prior rows treats every kind as due
/// immediately rather than waiting out a full cadence with nothing
/// scheduled at all.
async fn seed_last_enqueued(
    db: &Cratestack,
    sys: &CoolContext,
    specs: &[RecurringJobSpec],
) -> HashMap<&'static str, Option<DateTime<Utc>>> {
    let mut seeded = HashMap::with_capacity(specs.len());
    for spec in specs {
        let most_recent = db
            .job()
            .find_many()
            .where_expr(FilterExpr::from(job::kind().eq(spec.kind.to_owned())))
            .order_by(job::createdAt().desc())
            .limit(1)
            .run(sys)
            .await;

        let at = match most_recent {
            Ok(rows) => rows.into_iter().next().map(|row| row.createdAt),
            Err(error) => {
                warn!(
                    kind = spec.kind,
                    %error,
                    "seeding last-enqueued time failed; treating as never scheduled"
                );
                None
            }
        };
        seeded.insert(spec.kind, at);
    }
    seeded
}

/// One poll iteration. `pub` for the same reason `dispatch::tick` and
/// `jobs::tick` are — live tests drive exactly one iteration
/// deterministically instead of racing [`run`]'s own timer.
// Not generic over `BuildHasher`: this crate's only caller (`run`, and
// live tests exercising exactly what `run` does) always uses the default
// `HashMap`, and there's no reason a real deployment ever would swap it.
#[allow(clippy::implicit_hasher)]
pub async fn tick(
    db: &Cratestack,
    sys: &CoolContext,
    specs: &[RecurringJobSpec],
    last_enqueued: &mut HashMap<&'static str, Option<DateTime<Utc>>>,
) {
    let now = Utc::now();
    for spec in specs {
        let due = match last_enqueued.get(spec.kind).copied().flatten() {
            Some(at) => now - at >= spec.cadence,
            None => true,
        };
        if !due {
            continue;
        }

        match try_enqueue(db, sys, spec, now).await {
            // `Ok(false)` means a non-terminal instance of this kind
            // already exists — not due again until it clears. Recording
            // `now` either way stops this from retrying every tick until
            // the cadence window rolls past it naturally.
            Ok(_) => {
                last_enqueued.insert(spec.kind, Some(now));
            }
            Err(error) => {
                error!(kind = spec.kind, %error, "enqueuing a recurring job failed; retrying next tick");
            }
        }
    }
}

/// `Ok(true)` — enqueued. `Ok(false)` — a non-terminal instance of this
/// `kind` already exists (`jobs_dedupe_idx`), skip. See the module doc for
/// why `Ok(false)` is the documented outcome, not necessarily today's live
/// one.
async fn try_enqueue(
    db: &Cratestack,
    sys: &CoolContext,
    spec: &RecurringJobSpec,
    now: DateTime<Utc>,
) -> Result<bool, CoolError> {
    match db
        .job()
        .create(CreateJobInput {
            kind: spec.kind.to_owned(),
            dedupeKey: Some(spec.kind.to_owned()),
            payload: "{}".to_owned(),
            priority: spec.priority,
            runAt: now,
            leaseOwner: None,
            leaseUntil: None,
            maxAttempts: spec.max_attempts,
            lastError: None,
            startedAt: None,
            finishedAt: None,
        })
        .run(sys)
        .await
    {
        Ok(_) => Ok(true),
        Err(error) if error.db_sqlstate() == Some(UNIQUE_VIOLATION) => Ok(false),
        Err(error) => Err(error),
    }
}
