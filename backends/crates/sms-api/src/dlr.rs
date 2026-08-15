#![doc = include_str!("dlr.md")]

use chrono::{Duration as ChronoDuration, Utc};
use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_provider::{DeliveryOutcome, DeliveryUpdate, ProviderError, RawCallback, SmsProvider};
use tracing::{info, warn};

use crate::errors::{is_illegal_transition, map_database_error};
use crate::procedures::parse_operator_code;
use crate::schema::{self, Cratestack, MessageState, message};

/// §7.4's own backoff schedule for retryable failures ("Backoff on
/// retryable failures: 5s, 30s, 2m, 10m, 30m, capped by `maxAttempts` and
/// hard-stopped by `expiresAt`") — the exact same five values
/// `backends/crates/sms-worker/src/jobs.rs`'s own `BACKOFF_SCHEDULE` already reuses
/// for `Job` retries. Duplicated here rather than imported: the dependency
/// arrow only points from `app/` into `crates/`, and `sms-worker` already
/// depends on `sms-api` (for its generated schema), so `sms-api` cannot
/// depend back on `sms-worker` without a cycle. These are the same *values
/// from the spec*, reused independently in each crate that needs them —
/// exactly what `jobs.rs`'s own comment already documents for its side of
/// this, not a new drift risk.
const UNDELIVERED_BACKOFF_SCHEDULE: [ChronoDuration; 5] = [
    ChronoDuration::seconds(5),
    ChronoDuration::seconds(30),
    ChronoDuration::minutes(2),
    ChronoDuration::minutes(10),
    ChronoDuration::minutes(30),
];

/// The backoff to apply before a message that just landed in `undelivered`
/// becomes retry-eligible again, keyed by how many submission attempts it
/// has already made. `attempts` is always >= 1 by the time a message can
/// reach `undelivered` (`queued -> routed` increments it before any submit
/// is attempted), so this indexes the same way
/// `backends/crates/sms-worker/src/jobs.rs::backoff_for` does.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn undelivered_retry_backoff(attempts: i64) -> ChronoDuration {
    let index = (attempts - 1).max(0) as usize;
    UNDELIVERED_BACKOFF_SCHEDULE[index.min(UNDELIVERED_BACKOFF_SCHEDULE.len() - 1)]
}

/// Parse `raw` through `provider` and land each resulting update — never
/// fails the whole callback for one bad update; see [`ingest_one`]'s own
/// doc for why.
///
/// # Errors
///
/// Only when `provider.parse_dlr` itself rejects the callback body as
/// unparseable (`ProviderError::Rejected`) or unsupported
/// (`ProviderError::Unsupported`) — the caller maps this to an HTTP error
/// response. A per-update database error is logged, not propagated: see
/// [`ingest_one`].
pub async fn ingest(
    db: &Cratestack,
    sys: &CoolContext,
    provider: &dyn SmsProvider,
    provider_row_id: &str,
    raw: &RawCallback,
) -> Result<(), ProviderError> {
    let updates = provider.parse_dlr(raw)?;
    let raw_payload = String::from_utf8_lossy(&raw.body).into_owned();
    for update in updates {
        if let Err(error) = ingest_one(db, sys, provider_row_id, &raw_payload, &update).await {
            warn!(
                provider_ref = %update.provider_ref,
                %error,
                "ingesting one DLR update failed; continuing with the rest of the callback"
            );
        }
    }
    Ok(())
}

/// Match one update to its `Message`, write the `DeliveryReceipt`, and
/// drive the state transition it implies.
///
/// Errors from this function are logged by the caller, not propagated up
/// to the HTTP layer — a provider's callback can bundle several updates in
/// one request (Orange's own shape does), and one update referencing a
/// message we can't find, or landing a stale/out-of-order transition the
/// trigger correctly refuses, must not fail the ones around it.
///
/// # Errors
///
/// Any `CoolError` from the database reads/writes this performs, except a
/// rejected transition (`CoolError::Conflict`) on a *stale* update — see
/// the inline comment where that's handled — which resolves to `Ok(())`
/// after logging, since it is an expected outcome (a late or duplicate
/// DLR), not a fault.
async fn ingest_one(
    db: &Cratestack,
    sys: &CoolContext,
    provider_row_id: &str,
    raw_payload: &str,
    update: &DeliveryUpdate,
) -> Result<(), CoolError> {
    let candidates = db
        .message()
        .find_many()
        .where_expr(
            FilterExpr::from(message::providerId().eq(provider_row_id.to_owned())).and(
                FilterExpr::from(message::providerMessageRef().eq(update.provider_ref.clone()))
                    .or(message::providerMessageRefAlt().eq(update.provider_ref.clone())),
            ),
        )
        .limit(1)
        .run(sys)
        .await?;

    let Some(found) = candidates.into_iter().next() else {
        // Not an error: a provider can resend a DLR for a message that has
        // since aged out of retention, or (misconfigured webhook, a stale
        // whitelisted URL) reference a ref this deployment never issued.
        warn!(
            provider_ref = %update.provider_ref,
            "DLR references a provider_ref no known message currently has"
        );
        return Ok(());
    };

    let network = update
        .delivering_network
        .as_deref()
        .and_then(parse_operator_code)
        // The DLR didn't report a network — fall back to the message's own
        // prefix-based classification (§3.2's own routing pass), which is
        // a guess but the best one available. A DLR-reported network, when
        // a provider gives one, is real observed data and always wins —
        // see `DeliveryUpdate::delivering_network`'s own doc for why this
        // fallback direction, not the reverse, is what eventually
        // corrects `OperatorPrefixRule`.
        .unwrap_or(found.operator);

    db.delivery_receipt()
        .create(schema::CreateDeliveryReceiptInput {
            messageId: found.id.clone(),
            providerId: provider_row_id.to_owned(),
            providerMessageRef: update.provider_ref.clone(),
            outcome: to_schema_outcome(update.outcome),
            rawStatus: update.raw_status.clone(),
            errorCode: update.error_code.clone(),
            networkCode: network,
            occurredAt: update.occurred_at,
            rawPayload: raw_payload.to_owned(),
        })
        .run(sys)
        .await?;

    let Some(target) = next_state(found.state, update.outcome) else {
        // `Unknown` outcome (never guessed into a transition — see
        // `next_state`'s own doc), or no route from wherever this message
        // currently sits. The receipt just written still carries
        // `rawStatus` for a human to read.
        return Ok(());
    };
    if target == found.state {
        return Ok(());
    }

    // Only `undelivered` needs a fresh `leaseUntil` here — it's the one
    // non-terminal target this function ever proposes, and `claim.rs`'s
    // candidate query uses this same field, for this same message, as its
    // backoff gate (see the module doc). Every other target is either
    // terminal (no `candidates()` ever selects it again) or a same-state
    // no-op already returned above, so leaving `leaseUntil` untouched
    // (`None` — this is `Option<Option<_>>`, so `None` means "don't touch
    // the column") is correct for all of them.
    let lease_until = (target == MessageState::undelivered)
        .then(|| Some(Utc::now() + undelivered_retry_backoff(found.attempts)));

    let result = db
        .message()
        .update(found.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(target),
            leaseUntil: lease_until,
            ..Default::default()
        })
        .if_match(found.version)
        .run(sys)
        .await;

    match result {
        Ok(_) => {
            // #71: the second correlation event on this path — see
            // `procedures.rs`'s own module doc for the first. No
            // `cratestack_request_id` here: a DLR arrives on its own
            // unauthenticated HTTP connection (this module's own doc — no
            // bearer token, no `GatewayAuth`, so no `CoolContext` was ever
            // constructed per-request the way `sendMessage`'s was), so
            // there is no HTTP-request-scoped id to carry forward. The
            // join across processes is `message_id` alone.
            info!(
                message_id = %found.id,
                from_state = ?found.state,
                to_state = ?target,
                provider_ref = %update.provider_ref,
                "DLR applied"
            );
            Ok(())
        }
        // #71: checked against the *raw* error, before any mapping — same
        // reasoning as `backends/crates/sms-worker/src/jobs.rs::swallow_stale_write`
        // and `backends/crates/sms-worker/src/jobs/expire_stale.rs`'s own doc
        // comments give for the identical shape of fix, restated here
        // because this call site found the bug first, live, rather than by
        // reasoning about it in advance. `is_illegal_transition` reads
        // `error.db_sqlstate()` directly off the framework's own unmapped
        // error, so it doesn't need `map_database_error` to have already
        // run — and checking it *first* is what stops a genuine SM001 from
        // being caught by the `Err(CoolError::Conflict(reason))` arm below
        // and misreported as a merely stale DLR. That arm's own comment
        // used to claim `Conflict` here always meant "the message moved on
        // between the read and this write" — true for the version-mismatch
        // case (`PreconditionFailed`, matched below), but not for a
        // `next_state`/`message_state_transitions` drift bug: `if_match`
        // matching means the row's *actual* current state equals `found.
        // state`, exactly what `next_state` already computed `target` from
        // — so a real SM001 here can only mean the Rust-side transition
        // logic and the database's own transition table disagree about
        // what's legal, never a race. Confirmed live, not assumed: nothing
        // in `cratestack-sqlx` ever constructs a raw `CoolError::Conflict`
        // from this write path (see `update_run.rs`/`error.rs`) — before
        // this fix, the `Conflict` arm below was unreachable dead code,
        // and a real SM001 here silently fell through to `Err(error) =>
        // Err(error)` with the *wrong* message, not the swallowed-as-stale
        // outcome its own comment claimed.
        Err(error) if is_illegal_transition(&error) => Err(map_database_error(error)),
        // The message moved on (another DLR, an operator cancel) between
        // the read above and this write — an expected outcome of
        // at-least-once, possibly-reordered DLR delivery, not a fault: the
        // receipt is already written either way, so nothing about this
        // update is lost, just not applied to the message's own state.
        Err(CoolError::PreconditionFailed(reason)) => {
            warn!(
                message_id = %found.id,
                target = ?target,
                reason,
                "DLR-driven transition lost a race on its own version; likely a concurrent DLR \
                 or operator action"
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// The target `MessageState` a DLR outcome implies, given the message's
/// *current* state — not a fixed outcome-to-state table, because §7.4's
/// own transition table doesn't admit `uncertain -> undelivered` (§2.10):
/// a message that already spent time ambiguous doesn't get a further
/// retry on a later failure, it's terminal. See the `Failed` arm.
///
/// `None` for `DeliveryOutcome::Unknown` — per that variant's own doc, an
/// unclassifiable status must never be guessed into `Failed` or
/// `Delivered`. `None` also for any current state this function doesn't
/// name a target for; the caller's `if_match` + the trigger are the real
/// authority on legality regardless, this is just choosing the *intended*
/// target among what might be several legal ones.
fn next_state(current: MessageState, outcome: DeliveryOutcome) -> Option<MessageState> {
    match outcome {
        DeliveryOutcome::Delivered => Some(MessageState::delivered),
        DeliveryOutcome::Uncertain => Some(MessageState::uncertain),
        DeliveryOutcome::Expired => Some(MessageState::expired),
        DeliveryOutcome::Rejected => Some(MessageState::failed),
        DeliveryOutcome::Failed => match current {
            MessageState::submitted => Some(MessageState::undelivered),
            MessageState::uncertain => Some(MessageState::failed),
            _ => None,
        },
        DeliveryOutcome::Unknown => None,
    }
}

const fn to_schema_outcome(outcome: DeliveryOutcome) -> schema::DeliveryOutcome {
    match outcome {
        DeliveryOutcome::Delivered => schema::DeliveryOutcome::delivered,
        DeliveryOutcome::Uncertain => schema::DeliveryOutcome::uncertain,
        DeliveryOutcome::Failed => schema::DeliveryOutcome::failed,
        DeliveryOutcome::Expired => schema::DeliveryOutcome::expired,
        DeliveryOutcome::Rejected => schema::DeliveryOutcome::rejected,
        DeliveryOutcome::Unknown => schema::DeliveryOutcome::unknown,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration as ChronoDuration;

    use super::{next_state, to_schema_outcome, undelivered_retry_backoff};
    use schema::MessageState;
    use sms_provider::DeliveryOutcome;

    use crate::schema;

    #[test]
    fn undelivered_retry_backoff_follows_the_documented_schedule() {
        assert_eq!(undelivered_retry_backoff(1), ChronoDuration::seconds(5));
        assert_eq!(undelivered_retry_backoff(2), ChronoDuration::seconds(30));
        assert_eq!(undelivered_retry_backoff(3), ChronoDuration::minutes(2));
        assert_eq!(undelivered_retry_backoff(4), ChronoDuration::minutes(10));
        assert_eq!(undelivered_retry_backoff(5), ChronoDuration::minutes(30));
    }

    #[test]
    fn undelivered_retry_backoff_caps_at_the_last_schedule_entry() {
        assert_eq!(undelivered_retry_backoff(6), ChronoDuration::minutes(30));
        assert_eq!(undelivered_retry_backoff(1000), ChronoDuration::minutes(30));
    }

    #[test]
    fn undelivered_retry_backoff_never_panics_on_a_non_positive_attempts_value() {
        // Defensive only — `attempts` is a schema `Int @default(0)` and a
        // message can't reach `undelivered` with `attempts == 0` in
        // practice (a submit attempt is always counted first), but this
        // function must not panic if it somehow did.
        assert_eq!(undelivered_retry_backoff(0), ChronoDuration::seconds(5));
    }

    #[test]
    fn delivered_uncertain_expired_rejected_are_state_independent() {
        for current in [MessageState::submitted, MessageState::uncertain] {
            assert_eq!(
                next_state(current, DeliveryOutcome::Delivered),
                Some(MessageState::delivered)
            );
            assert_eq!(
                next_state(current, DeliveryOutcome::Expired),
                Some(MessageState::expired)
            );
            assert_eq!(
                next_state(current, DeliveryOutcome::Rejected),
                Some(MessageState::failed)
            );
        }
    }

    #[test]
    fn a_retryable_failure_from_submitted_goes_to_undelivered() {
        assert_eq!(
            next_state(MessageState::submitted, DeliveryOutcome::Failed),
            Some(MessageState::undelivered)
        );
    }

    #[test]
    fn a_retryable_failure_from_uncertain_goes_to_failed_not_undelivered() {
        // The whole reason next_state is current-state-aware: §2.10's
        // transition table has no `uncertain -> undelivered` edge.
        assert_eq!(
            next_state(MessageState::uncertain, DeliveryOutcome::Failed),
            Some(MessageState::failed)
        );
    }

    #[test]
    fn unknown_outcome_never_proposes_a_transition() {
        for current in [MessageState::submitted, MessageState::uncertain] {
            assert_eq!(next_state(current, DeliveryOutcome::Unknown), None);
        }
    }

    #[test]
    fn a_failure_from_an_unrelated_state_proposes_nothing() {
        assert_eq!(
            next_state(MessageState::delivered, DeliveryOutcome::Failed),
            None
        );
    }

    #[test]
    fn outcome_mapping_covers_every_variant() {
        assert_eq!(
            to_schema_outcome(DeliveryOutcome::Delivered),
            schema::DeliveryOutcome::delivered
        );
        assert_eq!(
            to_schema_outcome(DeliveryOutcome::Uncertain),
            schema::DeliveryOutcome::uncertain
        );
        assert_eq!(
            to_schema_outcome(DeliveryOutcome::Failed),
            schema::DeliveryOutcome::failed
        );
        assert_eq!(
            to_schema_outcome(DeliveryOutcome::Expired),
            schema::DeliveryOutcome::expired
        );
        assert_eq!(
            to_schema_outcome(DeliveryOutcome::Rejected),
            schema::DeliveryOutcome::rejected
        );
        assert_eq!(
            to_schema_outcome(DeliveryOutcome::Unknown),
            schema::DeliveryOutcome::unknown
        );
    }
}
