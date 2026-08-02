//! DLR ingestion and provider message-ref matching. #34.
//!
//! Mounted as a raw route (`POST /dlr/{providerKey}`, §7 of the design
//! doc), not through `CrateStack`'s generated router — a provider webhook
//! carries no bearer token to validate against `GatewayAuth`, so it can't
//! go through the same auth path as every other route.
//! `app/sms-gateway`'s own `dlr.rs` owns that route and the provider-key
//! dispatch; this module owns matching an already-parsed
//! [`sms_provider::DeliveryUpdate`] against a `Message` and driving the
//! state machine from it.
//!
//! # Scope
//!
//! Landing a message in `delivered`/`uncertain`/`undelivered`/`failed`/
//! `expired` from a DLR is the whole of this module. What happens *after*
//! `undelivered` — `undelivered -> queued: retry` (§7.4) — is a separate,
//! not-yet-built concern: `dispatch`'s own claim (`crates/sms-worker/src/claim.rs`)
//! only selects `accepted`/`queued`/`routed`, so nothing currently picks an
//! `undelivered` message back up. Tracked as a known gap, not silently
//! assumed to work.

use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_provider::{DeliveryOutcome, DeliveryUpdate, ProviderError, RawCallback, SmsProvider};
use tracing::warn;

use crate::procedures::parse_operator_code;
use crate::schema::{self, message, Cratestack, MessageState};

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

    match db
        .message()
        .update(found.id.clone())
        .set(schema::UpdateMessageInput {
            state: Some(target),
            ..Default::default()
        })
        .if_match(found.version)
        .run(sys)
        .await
    {
        Ok(_) => Ok(()),
        // The transition table (§2.10) rejected this — the message moved
        // on (another DLR, an operator cancel) between the read above and
        // this write, or a late/out-of-order DLR is proposing a transition
        // that's no longer legal from the message's *current* state. Both
        // are expected outcomes of at-least-once, possibly-reordered DLR
        // delivery, not a fault: the receipt is already written either
        // way, so nothing about this update is lost, just not applied to
        // the message's own state.
        Err(CoolError::Conflict(reason)) => {
            warn!(
                message_id = %found.id,
                target = ?target,
                reason,
                "DLR-driven transition was no longer legal; likely a stale or reordered DLR"
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
    use super::{next_state, to_schema_outcome};
    use schema::MessageState;
    use sms_provider::DeliveryOutcome;

    use crate::schema;

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
