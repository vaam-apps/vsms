#![doc = include_str!("claim.md")]

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::schema::{
    AttemptState, Cratestack, Job, JobState, Message, MessageState, UpdateJobInput,
    UpdateMessageInput, UpdateWebhookAttemptInput, WebhookAttempt, WebhookDeliveryMode,
    WebhookEndpoint, job, message, webhook_attempt, webhook_endpoint,
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
        sys: &CratestackContext,
        now: DateTime<Utc>,
        budget: i64,
    ) -> Result<Vec<Self>, CratestackError>;

    /// Attempt to take the lease on this one candidate via
    /// `if_match(self's version)` — the compare-and-swap itself. Must set
    /// whatever fields mean "claimed" for this model (a state transition,
    /// `leaseOwner`, `leaseUntil`, an attempts increment); [`claim_batch`]
    /// only interprets the *outcome*, never which fields changed.
    async fn take_lease(
        &self,
        db: &Cratestack,
        sys: &CratestackContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CratestackError>;
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
    sys: &CratestackContext,
    worker: &str,
    budget: i64,
) -> Result<Vec<C>, CratestackError> {
    let now = Utc::now();
    let candidates = C::candidates(db, sys, now, budget).await?;
    lease_candidates(db, sys, worker, now, candidates).await
}

/// [`claim_batch`]'s loop over candidates already selected: take each lease,
/// tell a lost race from a denied write from a real failure. One function so a
/// claim with its own candidate selection ([`claim_webhook_attempts`]) cannot
/// grow its own reading of those three outcomes.
async fn lease_candidates<C: Claimable>(
    db: &Cratestack,
    sys: &CratestackContext,
    worker: &str,
    now: DateTime<Utc>,
    candidates: Vec<C>,
) -> Result<Vec<C>, CratestackError> {
    let mut claimed = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        // #71: mapped before this match ever inspects it — `take_lease`'s
        // own transitions are exactly the shape that produced #33's own
        // `accepted -> routed` bug, and `sms_api::map_database_error` is
        // `sms_sm001_total`'s one recording site (`backends/crates/sms-api/src/
        // errors.rs`), so an illegal edge here needs to pass through it to
        // be counted. Mapping first also means an actual SM001 now arrives
        // here as `CratestackError::Conflict`, not `CratestackError::DatabaseTyped` —
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
            Err(CratestackError::PreconditionFailed(_)) => {}
            Err(CratestackError::Forbidden(_)) => {
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
    sys: &CratestackContext,
    message: &Message,
) -> Result<sms_routing::Decision, CratestackError> {
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
    sys: &CratestackContext,
    message: &Message,
    worker: &str,
    now: DateTime<Utc>,
) -> Result<Message, CratestackError> {
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
    sys: &CratestackContext,
    id: String,
    version: i64,
    worker: &str,
    now: DateTime<Utc>,
    reason: String,
) -> Result<Message, CratestackError> {
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
        sys: &CratestackContext,
        now: DateTime<Utc>,
        budget: i64,
    ) -> Result<Vec<Self>, CratestackError> {
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
        sys: &CratestackContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CratestackError> {
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
        sys: &CratestackContext,
        now: DateTime<Utc>,
        budget: i64,
    ) -> Result<Vec<Self>, CratestackError> {
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
        sys: &CratestackContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CratestackError> {
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
    sys: &CratestackContext,
    candidates: Vec<WebhookAttempt>,
    now: DateTime<Utc>,
    budget: i64,
) -> Result<Vec<WebhookAttempt>, CratestackError> {
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
        .filter(|endpoint| endpoint_is_claimable(endpoint, now))
        .map(|endpoint| endpoint.id)
        .collect();

    let budget = usize::try_from(budget).unwrap_or(usize::MAX);
    Ok(candidates
        .into_iter()
        .filter(|attempt| healthy.contains(&attempt.endpointId))
        .take(budget)
        .collect())
}

/// Which `WebhookEndpoint.deliveryMode`s the claiming worker can deliver.
///
/// A `sealed` delivery needs the envelope signing key
/// (`--envelope-signing-key-file`); an `hmac` one needs nothing but the row. A
/// worker without the key must not **claim** a sealed attempt at all: claiming
/// writes the row (`delivering`, a lease, an attempt counted), and a worker that
/// then cannot send it has either failed an attempt that never reached the
/// endpoint or invented a bookkeeping state to hide that. So the mode is decided
/// where the rows are chosen, in the claim's own query, ahead of the over-fetch
/// window: a sealed backlog the worker will not take cannot crowd the `hmac` rows
/// it will out of that window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServedModes {
    /// The worker holds the signing key: `hmac` and `sealed` attempts alike.
    Both,
    /// The worker holds no signing key: `hmac` attempts only.
    HmacOnly,
}

/// Attempts waiting for their turn: `pending` (never tried) or `failed` (resting
/// out a backoff), whose `nextAttemptAt` has arrived. The due-list
/// `webhook_due_idx` (§2.10) indexes.
fn due_attempts(now: DateTime<Utc>) -> FilterExpr {
    FilterExpr::from(webhook_attempt::state().in_([AttemptState::pending, AttemptState::failed]))
        .and(
            FilterExpr::from(webhook_attempt::nextAttemptAt().is_null())
                .or(webhook_attempt::nextAttemptAt().lte(now)),
        )
}

/// A crashed worker's abandoned attempts: `delivering` with a lease that has run
/// out (`webhook_attempts_lease_reclaim_idx`).
fn expired_leases(now: DateTime<Utc>) -> FilterExpr {
    FilterExpr::from(webhook_attempt::state().eq(AttemptState::delivering))
        .and(webhook_attempt::leaseUntil().lt(now))
}

/// The attempts whose endpoint delivers in `mode`, as a correlated subquery on
/// `endpointId` the delegate renders itself (the endpoint's own read policy
/// applies inside it, as for any relation filter).
fn endpoint_delivers(mode: WebhookDeliveryMode) -> FilterExpr {
    webhook_attempt::endpoint().deliveryMode().eq(mode)
}

/// Whether a claim may take `endpoint`'s attempts at `now`: it is active and its
/// circuit is closed (never opened, or open until a time that has passed). The one
/// rule [`filter_by_endpoint_health`] and the keyless worker's choice of endpoints
/// ([`claimable_hmac_endpoints`]) share.
fn endpoint_is_claimable(endpoint: &WebhookEndpoint, now: DateTime<Utc>) -> bool {
    endpoint.active
        && endpoint
            .circuitOpenUntil
            .is_none_or(|open_until| open_until <= now)
}

/// The attempts of an active endpoint, as a correlated subquery on `endpointId`.
/// Half of [`endpoint_is_claimable`], for the count that cannot load rows: the
/// other half, the circuit, compares an optional timestamp, which a filter over a
/// relation does not offer (`is_null` is all it has for an optional field).
fn endpoint_is_active() -> FilterExpr {
    webhook_attempt::endpoint().active().eq(true)
}

/// [`Claimable::candidates`] for [`WebhookAttempt`], for a worker serving `served`.
async fn webhook_candidates(
    db: &Cratestack,
    sys: &CratestackContext,
    now: DateTime<Utc>,
    budget: i64,
    served: ServedModes,
) -> Result<Vec<WebhookAttempt>, CratestackError> {
    let fetch_budget = budget
        .saturating_mul(CANDIDATE_OVERFETCH_FACTOR)
        .max(budget);
    let candidates = match served {
        ServedModes::Both => both_modes_candidates(db, sys, now, fetch_budget).await?,
        ServedModes::HmacOnly => hmac_only_candidates(db, sys, now, fetch_budget).await?,
    };
    filter_by_endpoint_health(db, sys, candidates, now, budget).await
}

/// The oldest `fetch_budget` due attempts of any endpoint, then the oldest expired
/// leases to fill what is left: `webhook_due_idx` and
/// `webhook_attempts_lease_reclaim_idx`, each read in order and stopped by `LIMIT`.
async fn both_modes_candidates(
    db: &Cratestack,
    sys: &CratestackContext,
    now: DateTime<Utc>,
    fetch_budget: i64,
) -> Result<Vec<WebhookAttempt>, CratestackError> {
    let mut ready = db
        .webhook_attempt()
        .find_many()
        .where_expr(due_attempts(now))
        .order_by(webhook_attempt::nextAttemptAt().asc())
        .limit(fetch_budget)
        .run(sys)
        .await?;
    let remaining = fetch_budget - i64::try_from(ready.len()).unwrap_or(fetch_budget);
    if remaining > 0 {
        ready.extend(
            db.webhook_attempt()
                .find_many()
                .where_expr(expired_leases(now))
                .order_by(webhook_attempt::leaseUntil().asc())
                .limit(remaining)
                .run(sys)
                .await?,
        );
    }
    Ok(ready)
}

/// The same selection for a worker with no signing key, whose cost follows the
/// `hmac` work and not the sealed backlog.
///
/// The obvious query, [`both_modes_candidates`] plus "and the endpoint delivers
/// `hmac`" as a subquery on `endpointId`, is the slow one: Postgres cannot push the
/// subquery into `webhook_due_idx`, so it reads that index in time order and looks
/// every entry's endpoint up until it has `fetch_budget` `hmac` rows, which is the
/// whole sealed backlog when there are few of them (200 000 due sealed attempts
/// and five `hmac` ones: 13 883 buffers, a poll). So the endpoints come first, from
/// the small `webhook_endpoints` table, and each one's due attempts are asked for
/// with the endpoint as an equality, which `webhook_endpoint_due_idx
/// (endpoint_id, next_attempt_at)` answers in order from its first entry.
///
/// It is one query per endpoint, deliberately and not one `endpointId IN (...)`:
/// with a list the planner is free to read `webhook_due_idx` in time order and
/// filter, and it does when it believes the endpoints hold a few percent of the
/// table, which statistics over the whole table (history included) make it believe
/// of any endpoint with a long past. An equality has no such plan to prefer: the
/// per-endpoint index reads exactly the rows asked for, never more than the
/// time-ordered index would. The merge of the per-endpoint lists by `nextAttemptAt`
/// keeps the oldest-first order [`both_modes_candidates`] gives.
///
/// The expired leases are one `IN` query: `webhook_attempts_lease_reclaim_idx`
/// holds `delivering` rows only, so whatever plan Postgres picks reads at most the
/// attempts in flight, not the backlog (`claim.md` has the measurements).
async fn hmac_only_candidates(
    db: &Cratestack,
    sys: &CratestackContext,
    now: DateTime<Utc>,
    fetch_budget: i64,
) -> Result<Vec<WebhookAttempt>, CratestackError> {
    let endpoints = claimable_hmac_endpoints(db, sys, now).await?;
    if endpoints.is_empty() {
        return Ok(Vec::new());
    }

    let mut ready = Vec::new();
    for endpoint_id in &endpoints {
        ready.extend(
            db.webhook_attempt()
                .find_many()
                .where_expr(
                    due_attempts(now).and(webhook_attempt::endpointId().eq(endpoint_id.clone())),
                )
                .order_by(webhook_attempt::nextAttemptAt().asc())
                .limit(fetch_budget)
                .run(sys)
                .await?,
        );
    }
    // SQL puts a missing `nextAttemptAt` last; so does this.
    ready.sort_by_key(|attempt| (attempt.nextAttemptAt.is_none(), attempt.nextAttemptAt));
    ready.truncate(usize::try_from(fetch_budget).unwrap_or(usize::MAX));

    let remaining = fetch_budget - i64::try_from(ready.len()).unwrap_or(fetch_budget);
    if remaining > 0 {
        ready.extend(
            db.webhook_attempt()
                .find_many()
                .where_expr(expired_leases(now).and(webhook_attempt::endpointId().in_(endpoints)))
                .order_by(webhook_attempt::leaseUntil().asc())
                .limit(remaining)
                .run(sys)
                .await?,
        );
    }
    Ok(ready)
}

/// The ids of the `hmac` endpoints a claim may take attempts of now: [`endpoint_is_claimable`]
/// among `webhook_endpoints`, a table of a few rows per app.
async fn claimable_hmac_endpoints(
    db: &Cratestack,
    sys: &CratestackContext,
    now: DateTime<Utc>,
) -> Result<Vec<String>, CratestackError> {
    let endpoints = db
        .webhook_endpoint()
        .find_many()
        .where_expr(FilterExpr::from(
            webhook_endpoint::deliveryMode().eq(WebhookDeliveryMode::hmac),
        ))
        .run(sys)
        .await?;
    Ok(endpoints
        .into_iter()
        .filter(|endpoint| endpoint_is_claimable(endpoint, now))
        .map(|endpoint| endpoint.id)
        .collect())
}

/// Claim up to `budget` webhook attempts a worker serving `served` can deliver
/// ([`ServedModes`]). [`claim_batch`] for a [`WebhookAttempt`] claims
/// [`ServedModes::Both`]; `hooks` calls this with what its signing key allows.
///
/// # Errors
///
/// Whatever [`claim_batch`] returns.
pub async fn claim_webhook_attempts(
    db: &Cratestack,
    sys: &CratestackContext,
    worker: &str,
    budget: i64,
    served: ServedModes,
) -> Result<Vec<WebhookAttempt>, CratestackError> {
    let now = Utc::now();
    let candidates = webhook_candidates(db, sys, now, budget, served).await?;
    lease_candidates(db, sys, worker, now, candidates).await
}

/// How many `sealed` attempts are waiting for a worker that holds the signing key:
/// due, or abandoned by a crashed worker, whose endpoint delivers `sealed` and is
/// **active**. A worker without the key counts them to say so
/// ([`ServedModes::HmacOnly`] never claims one), with a `COUNT(*)` rather than the
/// rows, which carry payloads.
///
/// A deactivated endpoint's attempts are left out: no worker claims them, with the
/// key or without it, so they say nothing about the key, and counting them made a
/// deployment with one deactivated sealed endpoint warn for as long as its rows
/// exist. An endpoint whose circuit is open is counted: the circuit is a pause
/// that ends by itself, after which its attempts are exactly what a keyless worker
/// cannot take. (It is also what a filter over a relation cannot say: it offers
/// `is_null` for an optional timestamp and no comparison.)
///
/// # Errors
///
/// Whatever the aggregate returns.
pub async fn count_sealed_attempts_waiting(
    db: &Cratestack,
    sys: &CratestackContext,
    now: DateTime<Utc>,
) -> Result<i64, CratestackError> {
    db.webhook_attempt()
        .aggregate()
        .count()
        .where_expr(
            due_attempts(now)
                .or(expired_leases(now))
                .and(endpoint_delivers(WebhookDeliveryMode::sealed))
                .and(endpoint_is_active()),
        )
        .run(sys)
        .await
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
    /// could. This selects for a worker that can deliver both modes;
    /// [`claim_webhook_attempts`] is the one that takes the mode into account.
    async fn candidates(
        db: &Cratestack,
        sys: &CratestackContext,
        now: DateTime<Utc>,
        budget: i64,
    ) -> Result<Vec<Self>, CratestackError> {
        webhook_candidates(db, sys, now, budget, ServedModes::Both).await
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
        sys: &CratestackContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CratestackError> {
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
