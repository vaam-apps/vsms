//! The CAS claim loop every claiming role shares. §7.3 of the design doc.
//!
//! `SKIP LOCKED` is not expressible through the framework — verified by grep
//! across every crate and by compile error on `skip_locked()` (§7.3).
//! `.for_update()` exists but blocks rather than skips, which for a claim
//! loop means workers queuing behind each other instead of moving on. So
//! every claim here is optimistic CAS on `@version` instead: select
//! candidates, take a lease with `if_match(version)`, and read the outcome —
//! `PreconditionFailed` means another worker won a race that was always
//! going to have a loser, `Forbidden` means something worth knowing about
//! happened, and anything else is a real failure.
//!
//! [`Claimable`] is what makes [`claim_batch`] one function rather than one
//! per model — "the job and webhook claims are the same function with
//! different types" (§7.3). This module implements it for [`Message`]
//! (`dispatch`'s claim, based on §7.3's own worked example, with one
//! correction — see the doc comment on its `candidates` impl) and for
//! [`Job`] (`jobs`'s claim, #35). `WebhookAttempt` (M3 #40) adds its own
//! `impl Claimable` when that story lands, reusing this loop rather than
//! re-deriving it.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_api::schema::{
    job, message, provider, Cratestack, Job, JobState, Message, MessageState, Provider,
    ProviderState, UpdateJobInput, UpdateMessageInput,
};
use tracing::warn;

/// A model this crate knows how to claim.
///
/// One implementation per claiming role's model. [`claim_batch`]'s loop —
/// the part that actually matters, distinguishing a lost race from a denied
/// write from a real failure — is written exactly once against this trait;
/// everything model-specific (what "unclaimed" means, what fields "claimed"
/// sets) lives in the `impl`.
#[async_trait]
pub trait Claimable: Sized + Send {
    /// For logging only — never used to build a query, so this doesn't need
    /// to be the literal primary key type, just something worth printing
    /// when a claim is denied.
    fn id(&self) -> String;

    /// Select up to `budget` candidates: unclaimed, or claimed by a lease
    /// that has since expired (a crashed worker's abandoned row — no
    /// separate reaper for the happy path, per §7.3), highest-priority /
    /// oldest first. Entirely model-specific: what "unclaimed" and "priority
    /// order" mean is a property of the model, not of claiming in general.
    async fn candidates(
        db: &Cratestack,
        sys: &CoolContext,
        now: DateTime<Utc>,
        budget: i64,
    ) -> Result<Vec<Self>, CoolError>;

    /// Attempt to take the lease on this one candidate via
    /// `if_match(self's version)` — the compare-and-swap itself. Must set
    /// whatever fields mean "claimed" for this model (a state transition,
    /// `leaseOwner`, `leaseUntil`, an attempts increment); [`claim_batch`]
    /// only interprets the *outcome*, never which fields changed.
    async fn take_lease(
        &self,
        db: &Cratestack,
        sys: &CoolContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CoolError>;
}

/// Claim up to `budget` rows of `C`.
///
/// `budget` derives from the caller's remaining throughput allowance — a
/// provider's TPS cap for `dispatch`, a scale-to-N role's configured
/// concurrency for `jobs`/`hooks` — never a fixed constant (§7.3).
///
/// # The two details that carry the whole design (§7.3, verbatim reasoning)
///
/// **`PreconditionFailed` means another worker won the race.** The
/// framework renders `if_match` as `WHERE id = $1 AND version = $2`; a
/// zero-row result becomes this error. That is exactly the semantics wanted
/// from a competing-consumer queue — the loser learns it lost, cheaply, and
/// moves to the next candidate.
///
/// **`Forbidden` is ambiguous and must not be swallowed as "lost the
/// race."** The framework returns it both when the update's policy denies
/// *and* when the row is invisible or gone — both produce zero rows.
/// Under a `system` principal, which every claim loop runs under, neither
/// should ever happen, so this is logged rather than silently retried:
/// folding it into the race branch would hide a policy regression as
/// mysterious throughput loss.
pub async fn claim_batch<C: Claimable>(
    db: &Cratestack,
    sys: &CoolContext,
    worker: &str,
    budget: i64,
) -> Result<Vec<C>, CoolError> {
    let now = Utc::now();
    let candidates = C::candidates(db, sys, now, budget).await?;

    let mut claimed = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        match candidate.take_lease(db, sys, worker, now).await {
            Ok(row) => claimed.push(row),
            Err(CoolError::PreconditionFailed(_)) => {}
            Err(CoolError::Forbidden(_)) => {
                warn!(
                    id = candidate.id(),
                    worker, "claim forbidden — policy denied or row gone"
                );
            }
            Err(e) => return Err(e),
        }
    }
    Ok(claimed)
}

/// `dispatch`'s claim lease once a message actually reaches `routed` — the
/// state and duration are §7.3's own worked example, transcribed rather
/// than newly decided. See [`Claimable::take_lease`]'s impl below for why
/// this isn't the target state for every candidate; §7.3's own worked
/// example collapses `accepted`/`queued`/`routed` into one `-> routed`
/// write, but `message_state_transitions` (§2.10) has no `accepted ->
/// routed` edge — only `accepted -> queued` and `queued -> routed` — so
/// that write is illegal for an `accepted` row.
const DISPATCH_LEASE: Duration = Duration::minutes(2);

/// No routing rules engine exists yet (M5, #62) — for M2, "routing" an
/// `accepted` message is choosing the one active `Provider`, same
/// cheapest-active-provider placeholder `sendMessage`'s own
/// `estimate_cost()` already uses (`crates/sms-api/src/procedures.rs`).
async fn cheapest_active_provider(
    db: &Cratestack,
    sys: &CoolContext,
) -> Result<Option<Provider>, CoolError> {
    Ok(db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            provider::state().eq(ProviderState::active),
        ))
        .order_by(provider::costPerSegmentXaf().asc())
        .limit(1)
        .run(sys)
        .await?
        .into_iter()
        .next())
}

#[async_trait]
impl Claimable for Message {
    fn id(&self) -> String {
        self.id.clone()
    }

    async fn candidates(
        db: &Cratestack,
        sys: &CoolContext,
        now: DateTime<Utc>,
        budget: i64,
    ) -> Result<Vec<Self>, CoolError> {
        // `routed` belongs in this list, not just `accepted`/`queued` — §7.3's
        // own illustrative query omits it, but `messages_lease_reclaim_idx`
        // (§2.10, committed since milestone 0) is built `WHERE ... state IN
        // ('queued','routed')`, and `routed -> queued` is a legal edge in
        // §7.4's own state machine. Without `routed` here, a worker that
        // crashes between claiming a message (accepted/queued -> routed) and
        // finishing it leaves that row permanently unreachable: no future
        // claim_batch call would ever see it again, contradicting this
        // trait's own contract ("reclaims rows abandoned by a crashed
        // worker... no separate reaper for the happy path") and the milestone
        // gate this loop exists to satisfy ("kill -9 the worker mid-submit
        // and the lease reclaims the message", #26/#36). A `routed` row only
        // ever exists with `leaseUntil` set (take_lease always sets it), so
        // the `leaseUntil IS NULL` branch below never matches for it — only
        // the expiry branch does, which is exactly the reclaim this is for.
        // Re-claiming sets state back to `routed`, a same-state assignment
        // the guard trigger always permits (`NEW.state IS NOT DISTINCT FROM
        // OLD.state` short-circuits before the transition-table check).
        db.message()
            .find_many()
            .where_expr(
                FilterExpr::from(message::state().in_([
                    MessageState::accepted,
                    MessageState::queued,
                    MessageState::routed,
                ]))
                .and(message::expiresAt().gt(now))
                .and(
                    FilterExpr::from(message::scheduledAt().is_null())
                        .or(message::scheduledAt().lte(now)),
                )
                .and(
                    FilterExpr::from(message::leaseUntil().is_null())
                        .or(message::leaseUntil().lt(now)),
                ),
            )
            .order_by(message::priority().desc())
            .order_by(message::createdAt().asc())
            .limit(budget)
            .run(sys)
            .await
    }

    /// What "claimed" means depends on which state the candidate arrived
    /// in — the single `-> routed` write §7.3 illustrates only actually
    /// applies to two of this candidate list's three states:
    ///
    /// - `accepted`: the routing pass (§7.4: "passes routing"). Picks the
    ///   one active provider and stamps `providerId`, or — no active
    ///   provider at all is a real, operator-visible outcome, not a
    ///   silent stall — transitions straight to `rejected`. Either way
    ///   this is an instant decision, not in-flight work, so it takes no
    ///   real lease: `leaseUntil` is left at `now`, already expired, so
    ///   the row is immediately eligible for the *next* claim rather than
    ///   blocked behind a full [`DISPATCH_LEASE`] it never needed.
    /// - `queued`: the actual dispatch claim. If `attempts` already
    ///   reached `maxAttempts` (set by a previous `routed -> queued`
    ///   backoff, see the dispatch loop), §7.4's `queued -> failed: max
    ///   attempts` edge applies — no further attempt is made. Otherwise
    ///   `-> routed`, `attempts` incremented (this is the one place a
    ///   submission attempt is counted — the routing pass above isn't
    ///   one), a real [`DISPATCH_LEASE`].
    /// - `routed`: a reclaim of a lease abandoned by a crashed worker.
    ///   Same-state write, `attempts` untouched (resuming the same
    ///   attempt already counted, not starting a new one), lease renewed.
    async fn take_lease(
        &self,
        db: &Cratestack,
        sys: &CoolContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CoolError> {
        match self.state {
            MessageState::accepted => match cheapest_active_provider(db, sys).await? {
                Some(provider) => {
                    db.message()
                        .update(self.id.clone())
                        .set(UpdateMessageInput {
                            state: Some(MessageState::queued),
                            providerId: Some(Some(provider.id)),
                            leaseOwner: Some(Some(worker.to_owned())),
                            leaseUntil: Some(Some(now)),
                            ..Default::default()
                        })
                        .if_match(self.version)
                        .run(sys)
                        .await
                }
                None => {
                    db.message()
                        .update(self.id.clone())
                        .set(UpdateMessageInput {
                            state: Some(MessageState::rejected),
                            stateReason: Some(Some("no active provider".to_owned())),
                            leaseOwner: Some(Some(worker.to_owned())),
                            leaseUntil: Some(Some(now)),
                            ..Default::default()
                        })
                        .if_match(self.version)
                        .run(sys)
                        .await
                }
            },
            MessageState::queued if self.attempts >= self.maxAttempts => {
                db.message()
                    .update(self.id.clone())
                    .set(UpdateMessageInput {
                        state: Some(MessageState::failed),
                        stateReason: Some(Some(format!(
                            "max attempts ({}) reached",
                            self.maxAttempts
                        ))),
                        leaseOwner: Some(Some(worker.to_owned())),
                        leaseUntil: Some(Some(now)),
                        ..Default::default()
                    })
                    .if_match(self.version)
                    .run(sys)
                    .await
            }
            MessageState::queued | MessageState::routed => {
                db.message()
                    .update(self.id.clone())
                    .set(UpdateMessageInput {
                        state: Some(MessageState::routed),
                        attempts: Some(if self.state == MessageState::queued {
                            self.attempts + 1
                        } else {
                            self.attempts
                        }),
                        leaseOwner: Some(Some(worker.to_owned())),
                        leaseUntil: Some(Some(now + DISPATCH_LEASE)),
                        ..Default::default()
                    })
                    .if_match(self.version)
                    .run(sys)
                    .await
            }
            other => {
                unreachable!("candidates() only returns accepted/queued/routed, got {other:?}")
            }
        }
    }
}

/// How long a claimed job holds its lease before it's eligible for
/// crash-reclaim. Generic across every `kind`, unlike [`DISPATCH_LEASE`]'s
/// single provider-call duration — a conservative fixed value rather than
/// per-kind config, since nothing about `jobs`'s own claim discipline
/// varies by kind and per-kind tuning can be added later without touching
/// this trait.
const JOB_LEASE: Duration = Duration::minutes(5);

#[async_trait]
impl Claimable for Job {
    fn id(&self) -> String {
        self.id.clone()
    }

    async fn candidates(
        db: &Cratestack,
        sys: &CoolContext,
        now: DateTime<Utc>,
        budget: i64,
    ) -> Result<Vec<Self>, CoolError> {
        // Two disjoint groups, matching the two partial indexes §2.10
        // already commits to (`jobs_claim_idx` on `pending`,
        // `jobs_lease_reclaim_idx` on `running`) — a single query spanning
        // both states with one shared `leaseUntil` predicate, the way
        // Message's own `candidates()` does, would use neither index for
        // its `pending` half (a `pending` row's `leaseUntil` is always
        // `NULL`, so an `OR` across states can't be satisfied by either
        // partial index alone). `take_lease` tells the two groups apart by
        // `self.state`, same as Message's does for its own three states.
        let mut pending = db
            .job()
            .find_many()
            .where_expr(
                FilterExpr::from(job::state().eq(JobState::pending)).and(job::runAt().lte(now)),
            )
            .order_by(job::priority().desc())
            .order_by(job::runAt().asc())
            .limit(budget)
            .run(sys)
            .await?;

        let remaining = budget - i64::try_from(pending.len()).unwrap_or(budget);
        if remaining > 0 {
            let reclaimable = db
                .job()
                .find_many()
                .where_expr(
                    FilterExpr::from(job::state().eq(JobState::running))
                        .and(job::leaseUntil().lt(now)),
                )
                .order_by(job::priority().desc())
                .order_by(job::runAt().asc())
                .limit(remaining)
                .run(sys)
                .await?;
            pending.extend(reclaimable);
        }
        Ok(pending)
    }

    /// Unlike Message's `routed` reclaim (a same-state write that resumes
    /// the same in-flight attempt), a crashed job's lease reclaim targets
    /// `running -> pending` — §7.5's own diagram edge, not an invented
    /// same-state trick. A generic job handler has no equivalent of "we
    /// already told the provider, don't double-submit"; the safe default
    /// is "nothing is known to have completed, so re-queue it" rather than
    /// assuming it's safe to resume mid-flight. Concretely this means a
    /// reclaimed job takes **two** [`claim_batch`] calls to actually run
    /// again: this one requeues it to `pending`, the *next* one (this tick
    /// or a later one) claims it for real via the `pending` branch below.
    /// Callers must filter a batch's results to `state == running` before
    /// treating a row as "claimed, go run it" — a `pending` result means
    /// "just requeued, not yet actually claimed."
    async fn take_lease(
        &self,
        db: &Cratestack,
        sys: &CoolContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CoolError> {
        match self.state {
            JobState::pending => {
                db.job()
                    .update(self.id.clone())
                    .set(UpdateJobInput {
                        state: Some(JobState::running),
                        attempts: Some(self.attempts + 1),
                        leaseOwner: Some(Some(worker.to_owned())),
                        leaseUntil: Some(Some(now + JOB_LEASE)),
                        ..Default::default()
                    })
                    .if_match(self.version)
                    .run(sys)
                    .await
            }
            JobState::running => {
                db.job()
                    .update(self.id.clone())
                    .set(UpdateJobInput {
                        state: Some(JobState::pending),
                        leaseOwner: Some(None),
                        leaseUntil: Some(None),
                        ..Default::default()
                    })
                    .if_match(self.version)
                    .run(sys)
                    .await
            }
            other => {
                unreachable!("candidates() only returns pending/running, got {other:?}")
            }
        }
    }
}
