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
//! correction — see the doc comment on its `candidates` impl); `Job` (#35)
//! and `WebhookAttempt` (M3 #40) each add their own `impl Claimable` when
//! their stories land, reusing this loop rather than re-deriving it.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_api::schema::{message, Cratestack, Message, MessageState, UpdateMessageInput};
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

/// `dispatch`'s claim: `accepted`/`queued`/`routed` → `routed`, a 2-minute
/// lease. The state and duration are §7.3's own worked example, transcribed
/// rather than newly decided — with one correction to the candidate state
/// list; see the doc comment on `candidates` below.
const DISPATCH_LEASE: Duration = Duration::minutes(2);

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

    async fn take_lease(
        &self,
        db: &Cratestack,
        sys: &CoolContext,
        worker: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, CoolError> {
        db.message()
            .update(self.id.clone())
            .set(UpdateMessageInput {
                state: Some(MessageState::routed),
                attempts: Some(self.attempts + 1),
                leaseOwner: Some(Some(worker.to_owned())),
                leaseUntil: Some(Some(now + DISPATCH_LEASE)),
                ..Default::default()
            })
            .if_match(self.version)
            .run(sys)
            .await
    }
}
