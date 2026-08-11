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
//! (`dispatch`'s claim, based on §7.3's own worked example, with two
//! corrections — see the doc comment on its `candidates` impl), for [`Job`]
//! (`jobs`'s claim, #35), and for [`WebhookAttempt`] (`hooks`'s claim, M3
//! #40 — see that `impl`'s own doc for how it differs from the other two:
//! endpoint health, not just row state, decides what's claimable).

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_api::schema::{
    job, message, webhook_attempt, webhook_endpoint, AttemptState, Cratestack, Job, JobState,
    Message, MessageState, UpdateJobInput, UpdateMessageInput, UpdateWebhookAttemptInput,
    WebhookAttempt,
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
        // #71: mapped before this match ever inspects it — `take_lease`'s
        // own transitions are exactly the shape that produced #33's own
        // `accepted -> routed` bug, and `sms_api::map_database_error` is
        // `sms_sm001_total`'s one recording site (`crates/sms-api/src/
        // errors.rs`), so an illegal edge here needs to pass through it to
        // be counted. Mapping first also means an actual SM001 now arrives
        // here as `CoolError::Conflict`, not `CoolError::DatabaseTyped` —
        // still falls to the `Err(e) => return Err(e)` arm below either
        // way (this loop has no "swallow a Conflict" branch of its own,
        // unlike `crate::jobs`/`crate::jobs::expire_stale`), but now
        // correctly counted on the way out.
        match candidate
            .take_lease(db, sys, worker, now)
            .await
            .map_err(sms_api::map_database_error)
        {
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

/// `accepted` message routing, since #62 — real `Route`-rule matching
/// (priority, weight, operator/class/app/prefix predicates) through
/// [`crate::routing::decide`], replacing the old `cheapest_active_provider`
/// M2 placeholder (still visible in git history / #33's own commit for
/// anyone comparing). No I/O happens inside the selection algorithm
/// itself — see `crate::routing`'s and `sms_routing`'s own module docs.
async fn route(
    db: &Cratestack,
    sys: &CoolContext,
    message: &Message,
) -> Result<sms_routing::Decision, CoolError> {
    let candidate = crate::routing::Candidate {
        operator: message.operator,
        class: message.class,
        app_id: &message.appId,
        msisdn: &message.msisdn,
        message_id: &message.id,
    };
    crate::routing::decide(db, sys, &candidate, &sms_routing::ExcludedRouteIds::new()).await
}

/// The `accepted` branch of `take_lease` — pulled out mainly to keep
/// `take_lease` itself under clippy's line-count limit (same reasoning as
/// [`fail_max_attempts`] above), not because it stands alone conceptually.
/// A winning [`route`] decision stamps both `providerId` and `routeId` and
/// moves to `queued`; no winner at all writes `rejected` with
/// [`crate::routing::explain_no_route`]'s own summary as `stateReason`.
/// Either way this is an instant decision, not in-flight work — see
/// [`Claimable::take_lease`]'s own doc on why no real lease is taken here.
async fn apply_routing_decision(
    db: &Cratestack,
    sys: &CoolContext,
    message: &Message,
    worker: &str,
    now: DateTime<Utc>,
) -> Result<Message, CoolError> {
    let decision = route(db, sys, message).await?;
    match decision.winner {
        Some(winner) => {
            db.message()
                .update(message.id.clone())
                .set(UpdateMessageInput {
                    state: Some(MessageState::queued),
                    providerId: Some(Some(winner.provider_id)),
                    routeId: Some(Some(winner.route_id)),
                    leaseOwner: Some(Some(worker.to_owned())),
                    leaseUntil: Some(Some(now)),
                    ..Default::default()
                })
                .if_match(message.version)
                .run(sys)
                .await
        }
        None => {
            db.message()
                .update(message.id.clone())
                .set(UpdateMessageInput {
                    state: Some(MessageState::rejected),
                    stateReason: Some(Some(crate::routing::explain_no_route(&decision))),
                    leaseOwner: Some(Some(worker.to_owned())),
                    leaseUntil: Some(Some(now)),
                    ..Default::default()
                })
                .if_match(message.version)
                .run(sys)
                .await
        }
    }
}

/// Shared "-> failed: max attempts" write for both the `queued` and
/// `undelivered` branches of `take_lease` below — same target state, same
/// lease bookkeeping, different only in the human-readable `reason`. Pulled
/// out mainly to keep `take_lease` itself under clippy's line-count limit
/// now that it has two "max attempts" arms instead of one (#122).
async fn fail_max_attempts(
    db: &Cratestack,
    sys: &CoolContext,
    id: String,
    version: i64,
    worker: &str,
    now: DateTime<Utc>,
    reason: String,
) -> Result<Message, CoolError> {
    db.message()
        .update(id)
        .set(UpdateMessageInput {
            state: Some(MessageState::failed),
            stateReason: Some(Some(reason)),
            leaseOwner: Some(Some(worker.to_owned())),
            leaseUntil: Some(Some(now)),
            ..Default::default()
        })
        .if_match(version)
        .run(sys)
        .await
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
        //
        // `undelivered` belongs here too, as of #122: a retryable DLR
        // failure (`submitted -> undelivered`, `sms_api::dlr::ingest_one`)
        // is exactly the kind of failure `undelivered -> queued: retry`
        // (§7.4, §2.10 — the edge has existed in `message_state_transitions`
        // since the initial schema commit) exists to drive, and until this
        // fix nothing ever selected `undelivered` here, so nothing ever
        // exercised that edge — the message just sat there forever (#122).
        // `dlr::ingest_one` stamps a backoff `leaseUntil` on that write
        // (§7.4's own "5s, 30s, 2m, 10m, 30m" schedule, keyed by
        // `attempts`), so the shared `leaseUntil` filter below is what holds
        // an `undelivered` row back from being retried immediately — the
        // exact same mechanism `routed -> queued`'s own backoff
        // (`dispatch::write_transition`) already relies on, not a new one
        // invented for this state.
        db.message()
            .find_many()
            .where_expr(
                FilterExpr::from(message::state().in_([
                    MessageState::accepted,
                    MessageState::queued,
                    MessageState::routed,
                    MessageState::undelivered,
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
    /// applies to two of this candidate list's four states:
    ///
    /// - `accepted`: the routing pass (§7.4: "passes routing"), since #62
    ///   real `Route`-rule matching through [`route`] — priority, weight,
    ///   operator/class/app/prefix predicates, provider availability, all
    ///   fully explained by the [`sms_routing::Decision`] it returns. A
    ///   winning route stamps both `providerId` and `routeId`; no eligible
    ///   route at all (no `Route` rows configured, or none matched) is a
    ///   real, operator-visible outcome, not a silent stall — transitions
    ///   straight to `rejected` with the decision's own explanation as
    ///   `stateReason` (`crate::routing::explain_no_route`). Either way
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
    /// - `undelivered`: a retry (#122). If `attempts` already reached
    ///   `maxAttempts`, `undelivered -> failed: max attempts` (§7.4)
    ///   applies directly — the same outcome the `queued` branch above
    ///   would reach anyway on its next claim, just without the extra,
    ///   pointless round trip through `queued` first. Otherwise
    ///   `-> queued`, `attempts` untouched (mirrors `accepted`'s branch:
    ///   this hop is a decision, not new in-flight work, so no real lease
    ///   is needed — the row is immediately eligible for the very next
    ///   claim). The actual attempt is still counted exactly once, at
    ///   `queued -> routed`, same as every other path into `queued`.
    async fn take_lease(
        &self,
        db: &Cratestack,
        sys: &CoolContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CoolError> {
        match self.state {
            MessageState::accepted => apply_routing_decision(db, sys, self, worker, now).await,
            MessageState::queued if self.attempts >= self.maxAttempts => {
                fail_max_attempts(
                    db,
                    sys,
                    self.id.clone(),
                    self.version,
                    worker,
                    now,
                    format!("max attempts ({}) reached", self.maxAttempts),
                )
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
            MessageState::undelivered if self.attempts >= self.maxAttempts => {
                fail_max_attempts(
                    db,
                    sys,
                    self.id.clone(),
                    self.version,
                    worker,
                    now,
                    format!(
                        "max attempts ({}) reached after a retryable delivery failure",
                        self.maxAttempts
                    ),
                )
                .await
            }
            MessageState::undelivered => {
                db.message()
                    .update(self.id.clone())
                    .set(UpdateMessageInput {
                        state: Some(MessageState::queued),
                        leaseOwner: Some(Some(worker.to_owned())),
                        leaseUntil: Some(Some(now)),
                        ..Default::default()
                    })
                    .if_match(self.version)
                    .run(sys)
                    .await
            }
            other => {
                unreachable!(
                    "candidates() only returns accepted/queued/routed/undelivered, got {other:?}"
                )
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

/// How long a claimed webhook attempt holds its lease before it's eligible
/// for crash-reclaim — comfortably above `hooks::REQUEST_TIMEOUT`'s own 10s
/// (§8.5) so an in-flight HTTP call is never reclaimed out from under
/// itself, with headroom for scheduling jitter. `hooks.rs` doesn't
/// re-export its own constant here to avoid a dependency edge from this
/// crate's claim mechanics back onto one role's poll loop — see that
/// module's own `REQUEST_TIMEOUT` doc for the value this must stay above.
const HOOKS_LEASE: Duration = Duration::seconds(30);

/// How many extra candidates [`Claimable::candidates`] fetches beyond
/// `budget`, so that filtering out attempts whose endpoint is inactive or
/// circuit-open (below) still tends to fill the batch rather than
/// under-running it every tick an unhealthy endpoint happens to dominate the
/// due queue. Not a correctness requirement — an under-filled batch just
/// means this tick claims fewer rows, which the next tick's due-list still
/// contains — only a throughput smoothing knob, same spirit as `dispatch`'s
/// own "not a fixed throughput guarantee" budget caveat (§7.3).
const CANDIDATE_OVERFETCH_FACTOR: i64 = 3;

/// Fetch the [`WebhookEndpoint`] rows named by `candidates`' `endpointId`s
/// and drop any candidate whose endpoint is inactive or has an open circuit
/// — §8.5: "stops attempting that endpoint... rows are still created as
/// `pending`, so nothing is lost — they're just not attempted." Applied
/// application-side rather than as a join in the candidate query itself:
/// `WebhookEndpoint` has no `@version`, so it plays no part in this claim's
/// own CAS — it is a coarser, best-effort filter over the candidate list,
/// not a second thing being claimed. Truncates to `budget` only after
/// filtering, so the caller never sees more than it asked for.
async fn filter_by_endpoint_health(
    db: &Cratestack,
    sys: &CoolContext,
    candidates: Vec<WebhookAttempt>,
    now: DateTime<Utc>,
    budget: i64,
) -> Result<Vec<WebhookAttempt>, CoolError> {
    if candidates.is_empty() {
        return Ok(candidates);
    }

    let mut endpoint_ids: Vec<String> = candidates.iter().map(|a| a.endpointId.clone()).collect();
    endpoint_ids.sort_unstable();
    endpoint_ids.dedup();

    let endpoints = db
        .webhook_endpoint()
        .find_many()
        .where_expr(FilterExpr::from(webhook_endpoint::id().in_(endpoint_ids)))
        .run(sys)
        .await?;

    let healthy: HashSet<String> = endpoints
        .into_iter()
        .filter(|endpoint| {
            endpoint.active
                && endpoint
                    .circuitOpenUntil
                    .is_none_or(|open_until| open_until <= now)
        })
        .map(|endpoint| endpoint.id)
        .collect();

    let budget = usize::try_from(budget).unwrap_or(usize::MAX);
    Ok(candidates
        .into_iter()
        .filter(|attempt| healthy.contains(&attempt.endpointId))
        .take(budget)
        .collect())
}

#[async_trait]
impl Claimable for WebhookAttempt {
    fn id(&self) -> String {
        self.id.clone()
    }

    /// Two "waiting" states feed the same due-list `webhook_due_idx` (§2.10)
    /// already indexes — `pending` (never yet attempted) and `failed`
    /// (attempted at least once, resting out its backoff, `nextAttemptAt`
    /// set by `hooks::write_outcome`) — plus a crash-reclaim group: a
    /// `delivering` row whose lease has expired, `webhook_attempts_lease_
    /// reclaim_idx`'s own reason to exist. Both groups are over-fetched
    /// (see [`CANDIDATE_OVERFETCH_FACTOR`]) and then narrowed by
    /// [`filter_by_endpoint_health`] before `budget` is actually applied —
    /// the query itself can't express "and the endpoint is healthy" without
    /// a join this schema's delegates don't offer, and `WebhookEndpoint`
    /// carries no `@version` for that join to claim against even if it
    /// could.
    async fn candidates(
        db: &Cratestack,
        sys: &CoolContext,
        now: DateTime<Utc>,
        budget: i64,
    ) -> Result<Vec<Self>, CoolError> {
        let fetch_budget = budget
            .saturating_mul(CANDIDATE_OVERFETCH_FACTOR)
            .max(budget);

        let mut ready = db
            .webhook_attempt()
            .find_many()
            .where_expr(
                FilterExpr::from(
                    webhook_attempt::state().in_([AttemptState::pending, AttemptState::failed]),
                )
                .and(
                    FilterExpr::from(webhook_attempt::nextAttemptAt().is_null())
                        .or(webhook_attempt::nextAttemptAt().lte(now)),
                ),
            )
            .order_by(webhook_attempt::nextAttemptAt().asc())
            .limit(fetch_budget)
            .run(sys)
            .await?;

        let remaining = fetch_budget - i64::try_from(ready.len()).unwrap_or(fetch_budget);
        if remaining > 0 {
            let reclaimable = db
                .webhook_attempt()
                .find_many()
                .where_expr(
                    FilterExpr::from(webhook_attempt::state().eq(AttemptState::delivering))
                        .and(webhook_attempt::leaseUntil().lt(now)),
                )
                .order_by(webhook_attempt::leaseUntil().asc())
                .limit(remaining)
                .run(sys)
                .await?;
            ready.extend(reclaimable);
        }

        filter_by_endpoint_health(db, sys, ready, now, budget).await
    }

    /// `pending`/`failed` both target `delivering` the same way — the only
    /// difference between them is whether this is the first attempt or a
    /// retry, and `attempts` (incremented here, exactly once per real
    /// attempt) already carries that distinction for `hooks::write_outcome`
    /// to read later. A `delivering` candidate is always the crash-reclaim
    /// case (`candidates` only ever selects one whose lease already
    /// expired): a same-state write that renews the lease without touching
    /// `attempts`, resuming the attempt already counted rather than
    /// asserting the customer endpoint never received the first one — the
    /// same reasoning `Message`'s own `routed` reclaim documents. Same-state
    /// writes bypass `attempts_guard_transition`'s table check entirely (its
    /// own early return), so this needs no row in `attempt_state_transitions`.
    async fn take_lease(
        &self,
        db: &Cratestack,
        sys: &CoolContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CoolError> {
        match self.state {
            AttemptState::pending | AttemptState::failed => {
                db.webhook_attempt()
                    .update(self.id.clone())
                    .set(UpdateWebhookAttemptInput {
                        state: Some(AttemptState::delivering),
                        attempts: Some(self.attempts + 1),
                        leaseOwner: Some(Some(worker.to_owned())),
                        leaseUntil: Some(Some(now + HOOKS_LEASE)),
                        ..Default::default()
                    })
                    .if_match(self.version)
                    .run(sys)
                    .await
            }
            AttemptState::delivering => {
                db.webhook_attempt()
                    .update(self.id.clone())
                    .set(UpdateWebhookAttemptInput {
                        state: Some(AttemptState::delivering),
                        leaseOwner: Some(Some(worker.to_owned())),
                        leaseUntil: Some(Some(now + HOOKS_LEASE)),
                        ..Default::default()
                    })
                    .if_match(self.version)
                    .run(sys)
                    .await
            }
            other => {
                unreachable!("candidates() only returns pending/failed/delivering, got {other:?}")
            }
        }
    }
}
