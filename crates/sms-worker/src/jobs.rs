//! `Role::Jobs`'s real body — #35. The generic background queue: claim,
//! dispatch by `kind` through [`JobHandler`], transition on the outcome
//! per §7.5's state machine.
//!
//! `Job` candidates arrive from [`crate::claim::claim_batch`] in one of two
//! states — see [`crate::claim::Claimable for Job`]'s own doc for why a
//! `pending` result means "just reclaimed, not actually claimed yet" and
//! must not be executed this tick.
//!
//! Only one [`JobHandler`] is registered for this milestone —
//! [`expire_stale`] — proving the pipeline end to end without depending on
//! infrastructure this milestone doesn't build (Orange balance/health
//! endpoints, audit anchoring, backup verification, and the still-open
//! retention-law question in the doc's own §"Open questions"). The other
//! eight `kind`s §7.5's own table names are real, tracked gaps, not a
//! silently dropped scope — see the module's own issue for the follow-up.

pub mod expire_stale;

use std::collections::HashMap;
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use cratestack::{CoolContext, CoolError};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{Cratestack, Job, JobState, UpdateJobInput};
use tracing::{error, warn};

use crate::claim::claim_batch;
use crate::WorkerContext;

/// How often this loop polls for claimable jobs. No TPS-style external
/// ceiling constrains `jobs` the way Orange's contract constrains
/// `dispatch` — this is just "responsive without hammering the database
/// every tick", same order of magnitude as `dispatch::POLL_INTERVAL`.
const POLL_INTERVAL: StdDuration = StdDuration::from_secs(1);

/// How many jobs one poll claims at once. `jobs` is scale-to-N (§7.1), so
/// unlike `dispatch`'s TPS-derived budget there's no external throughput
/// ceiling to size this against — a fixed, conservative batch, matching
/// the same "not a fixed constant" caveat `dispatch::budget_for`'s own doc
/// names for its role: real backpressure/concurrency tuning is future
/// work, not required for this milestone's single-handler pipeline.
const BUDGET: i64 = 10;

/// §7.4's own message backoff schedule, reused rather than inventing a
/// second one — §7.5 draws the `failed -> pending: backoff elapsed` edge
/// but never states a schedule of its own, and there's no reason a job
/// retry should back off faster or slower than a message one.
const BACKOFF_SCHEDULE: [Duration; 5] = [
    Duration::seconds(5),
    Duration::seconds(30),
    Duration::minutes(2),
    Duration::minutes(10),
    Duration::minutes(30),
];

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn backoff_for(attempts: i64) -> Duration {
    let index = (attempts - 1).max(0) as usize;
    BACKOFF_SCHEDULE[index.min(BACKOFF_SCHEDULE.len() - 1)]
}

/// A job's business logic for exactly one `kind`. `Send + Sync + 'static`
/// so a [`Registry`] can hold a `Box<dyn JobHandler>` — mirrors
/// [`sms_provider::SmsProvider`]'s own shape for the same reason: one
/// trait, many independent implementations, dispatched by a string key at
/// runtime rather than a compile-time enum (`Job.kind` is a free-form
/// `String`, not a schema enum — §7.5's table is documentation, not a
/// closed set the type system enforces).
#[async_trait]
pub trait JobHandler: Send + Sync + 'static {
    /// Must equal the `kind` this handler runs — [`Registry::register`]
    /// keys on it.
    fn kind(&self) -> &'static str;

    /// Do the work. `Err`'s `String` becomes `Job.lastError` verbatim — no
    /// retryable/permanent distinction like [`sms_provider::ProviderError`]
    /// makes, because §7.5's diagram draws exactly one failure edge
    /// (`running -> failed`) regardless of cause; every failure gets the
    /// same backoff-then-`dead` treatment.
    async fn run(&self, db: &Cratestack, sys: &CoolContext, job: &Job) -> Result<(), String>;
}

/// `kind` string to handler, built once at startup.
pub struct Registry {
    handlers: HashMap<&'static str, Box<dyn JobHandler>>,
}

impl Registry {
    /// An empty registry — every `kind` falls through to `run_one`'s
    /// "no handler registered" failure until [`Registry::register`] adds
    /// one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Add a handler, keyed by its own [`JobHandler::kind`]. Consumes and
    /// returns `self` so [`default_registry`] reads as a plain builder
    /// chain.
    #[must_use]
    pub fn register(mut self, handler: impl JobHandler) -> Self {
        self.handlers.insert(handler.kind(), Box::new(handler));
        self
    }

    fn get(&self, kind: &str) -> Option<&dyn JobHandler> {
        self.handlers.get(kind).map(std::convert::AsRef::as_ref)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// This milestone's registry — see the module doc for why only
/// `expire_stale` is wired up.
#[must_use]
pub fn default_registry() -> Registry {
    Registry::new().register(expire_stale::ExpireStale)
}

fn sys(worker: &str) -> CoolContext {
    Principal {
        sub: format!("sms-worker:jobs:{worker}"),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// Never returns on its own, matching [`crate::run`]'s contract.
pub async fn run(ctx: WorkerContext, worker: &str) {
    let sys = sys(worker);
    let registry = default_registry();
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    loop {
        interval.tick().await;
        if let Err(error) = tick(&ctx, &sys, worker, &registry).await {
            error!(%error, "jobs tick failed; retrying next poll");
        }
    }
}

/// One poll iteration. `pub` for the same reason `dispatch::tick` is —
/// live tests drive exactly one iteration deterministically.
pub async fn tick(
    ctx: &WorkerContext,
    sys: &CoolContext,
    worker: &str,
    registry: &Registry,
) -> Result<(), CoolError> {
    let claimed = claim_batch::<Job>(&ctx.db, sys, worker, BUDGET).await?;
    for job in claimed {
        // A `pending` result is a crash-reclaim, not a real claim — see
        // `Claimable for Job::take_lease`'s own doc. Nothing to execute
        // yet; the next claim (this tick or a later one) picks it up for
        // real via the `running` branch.
        if job.state == JobState::running {
            run_one(&ctx.db, sys, &job, registry).await;
        }
    }
    Ok(())
}

/// Run one already-`running` job and write back whichever transition its
/// outcome implies. Errors writing that transition are logged, not
/// propagated — one job's DB write failing must not stall the rest of this
/// tick's batch, same reasoning as `dispatch::submit_one`.
async fn run_one(db: &Cratestack, sys: &CoolContext, job: &Job, registry: &Registry) {
    let outcome = match registry.get(&job.kind) {
        Some(handler) => handler.run(db, sys, job).await,
        None => Err(format!("no handler registered for kind {:?}", job.kind)),
    };

    if let Err(error) = apply_outcome(db, sys, job, outcome).await {
        error!(job_id = %job.id, %error, "recording a job's outcome failed");
    }
}

/// Write the transition `outcome` implies, per §7.5: `succeeded` on `Ok`;
/// on `Err`, `running -> failed` and then immediately `failed -> pending`
/// (with backoff) or `failed -> dead` (attempts exhausted) — both legal
/// single-tick hops, same "propose several transitions from one outcome"
/// shape `crates/sms-api/src/dlr.rs`'s `ingest_one` already uses.
async fn apply_outcome(
    db: &Cratestack,
    sys: &CoolContext,
    job: &Job,
    outcome: Result<(), String>,
) -> Result<(), CoolError> {
    let message = match outcome {
        Ok(()) => {
            return match db
                .job()
                .update(job.id.clone())
                .set(UpdateJobInput {
                    state: Some(JobState::succeeded),
                    ..Default::default()
                })
                .if_match(job.version)
                .run(sys)
                .await
            {
                Ok(_) => Ok(()),
                Err(error) => swallow_stale_write(job, error),
            };
        }
        Err(message) => message,
    };
    apply_failure(db, sys, job, message).await
}

async fn apply_failure(
    db: &Cratestack,
    sys: &CoolContext,
    job: &Job,
    message: String,
) -> Result<(), CoolError> {
    let failed = match db
        .job()
        .update(job.id.clone())
        .set(UpdateJobInput {
            state: Some(JobState::failed),
            lastError: Some(Some(message)),
            ..Default::default()
        })
        .if_match(job.version)
        .run(sys)
        .await
    {
        Ok(row) => row,
        Err(error) => return swallow_stale_write(job, error),
    };

    let next = if failed.attempts >= failed.maxAttempts {
        UpdateJobInput {
            state: Some(JobState::dead),
            ..Default::default()
        }
    } else {
        UpdateJobInput {
            state: Some(JobState::pending),
            runAt: Some(Utc::now() + backoff_for(failed.attempts)),
            leaseOwner: Some(None),
            leaseUntil: Some(None),
            ..Default::default()
        }
    };

    match db
        .job()
        .update(failed.id.clone())
        .set(next)
        .if_match(failed.version)
        .run(sys)
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => swallow_stale_write(&failed, error),
    }
}

/// A `Conflict`/`PreconditionFailed` here means this job's lease expired
/// mid-run and another worker already reclaimed it (`running -> pending`,
/// per `Claimable for Job`) before this write landed — an expected race
/// under a slow handler, not a fault: the reclaiming worker's own later
/// claim will retry the job from scratch. Anything else propagates.
fn swallow_stale_write(job: &Job, error: CoolError) -> Result<(), CoolError> {
    match error {
        CoolError::Conflict(reason) => {
            warn!(
                job_id = %job.id,
                reason,
                "job outcome write raced a lease reclaim; the reclaiming worker will retry it"
            );
            Ok(())
        }
        CoolError::PreconditionFailed(reason) => {
            warn!(
                job_id = %job.id,
                reason,
                "job outcome write lost a race on its own version; the reclaiming worker will retry it"
            );
            Ok(())
        }
        other => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::{backoff_for, BACKOFF_SCHEDULE};
    use chrono::Duration;

    #[test]
    fn the_first_attempt_backs_off_by_the_schedules_first_entry() {
        assert_eq!(backoff_for(1), Duration::seconds(5));
    }

    #[test]
    fn each_attempt_walks_one_step_further_into_the_schedule() {
        let len = i64::try_from(BACKOFF_SCHEDULE.len()).unwrap();
        for (attempts, expected) in (1..=len).zip(BACKOFF_SCHEDULE) {
            assert_eq!(backoff_for(attempts), expected, "attempts={attempts}");
        }
    }

    #[test]
    fn attempts_past_the_schedules_length_stay_capped_at_the_last_entry() {
        let last = *BACKOFF_SCHEDULE.last().unwrap();
        let len = i64::try_from(BACKOFF_SCHEDULE.len()).unwrap();
        assert_eq!(backoff_for(len + 1), last);
        assert_eq!(backoff_for(1000), last);
    }
}
