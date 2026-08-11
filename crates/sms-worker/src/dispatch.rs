//! `Role::Dispatch`'s real body — #33. Drives `routed -> submitted` and
//! `routed`'s failure edges per §7.4; `accepted -> queued` (and its own
//! `-> rejected` edge) already lives in [`crate::claim`]'s `take_lease`,
//! since that hop is atomic with the claim itself, not a separate step
//! this loop drives.
//!
//! Polls rather than reacting to a notification, on a short interval —
//! comfortably under the M2 gate's <15s delivery target even allowing for
//! one full miss between a message becoming claimable and this loop
//! noticing.

use std::sync::Arc;
use std::time::Duration;

use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{provider, Encoding, Message, MessageState, UpdateMessageInput};
use sms_encoding::SmsEncoding;
use sms_provider::{ProviderError, SmsProvider, SubmitRequest};
use tracing::{error, warn};

use crate::claim::claim_batch;
use crate::{ProviderRegistry, WorkerContext};

/// How often this loop polls for claimable messages.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// `ProviderError::Unavailable` carries no `retry_after` of its own (§6.1:
/// it means the provider broadly, not this one message) — this crate picks
/// a conservative fixed backoff rather than retrying immediately.
const UNAVAILABLE_BACKOFF: Duration = Duration::from_secs(30);

/// The `system` context this role does all its work under — `kind` and
/// `role` both set, per the trap `#21`/`Principal::into_context`'s own doc
/// names: setting only one denies every write.
fn sys(worker: &str) -> CoolContext {
    Principal {
        sub: format!("sms-worker:dispatch:{worker}"),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// Never returns on its own, matching [`crate::run`]'s contract — the
/// caller (`run_singleton`) is what stops this, by dropping the future on
/// shutdown.
pub async fn run(ctx: WorkerContext, worker: &str) {
    let sys = sys(worker);
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    loop {
        interval.tick().await;
        if let Err(error) = tick(&ctx, &sys, worker).await {
            error!(%error, "dispatch tick failed; retrying next poll");
        }
    }
}

/// One poll iteration: claim, then submit whatever reached `routed`. `pub`
/// so live tests can drive exactly one iteration deterministically instead
/// of racing [`run`]'s own timer — the same reason [`crate::claim::claim_batch`]
/// itself is `pub` rather than only reachable through a role's loop.
pub async fn tick(ctx: &WorkerContext, sys: &CoolContext, worker: &str) -> Result<(), CoolError> {
    let budget = budget_for(total_tps_ceiling(&ctx.providers));
    let claimed = claim_batch::<Message>(&ctx.db, sys, worker, budget).await?;

    for message in claimed {
        // `queued` rows just routed this tick (the `accepted -> queued`
        // hop, done inside `take_lease`) need nothing further here — the
        // very next tick's `candidates()` picks them straight back up,
        // per A1's no-real-lease design for that hop. Only rows that
        // reached `routed` this claim are ready to actually submit.
        if message.state == MessageState::routed {
            submit_one(ctx, sys, message).await;
        }
    }
    Ok(())
}

/// `budget` derives from the provider's remaining TPS allowance, not a
/// fixed constant (§7.3) — sized to what one poll interval can spend
/// against `tps_ceiling` without a fractional tick being silently rounded
/// to zero.
fn budget_for(tps_ceiling: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let budget = (tps_ceiling * POLL_INTERVAL.as_secs_f64()).ceil() as i64;
    budget.max(1)
}

/// Sum of every registered provider's `tps_ceiling` — since #62, a
/// `routed` message may submit through any of them, not just one, so the
/// per-tick claim budget has to reflect the whole registry's combined
/// throughput allowance, not a single provider's. With exactly one
/// provider configured (every deployment today), this is identical to
/// that provider's own `tps_ceiling` — no behaviour change until a second
/// real adapter exists. Deliberately coarse: this does not give each
/// provider its own isolated share of the budget, so a burst of messages
/// routed to a slow/starved provider can still consume claim slots a
/// healthy provider could have used this tick. Real per-provider
/// throughput isolation is §6.3's own "remaining TPS/daily budget"
/// filtering, out of scope for #62 (see `crates/sms-routing`'s own module
/// doc) and a natural fit for #63.
fn total_tps_ceiling(providers: &ProviderRegistry) -> f64 {
    providers
        .values()
        .map(|provider| provider.capabilities().tps_ceiling)
        .sum()
}

fn decode_encoding(encoding: Encoding) -> SmsEncoding {
    match encoding {
        Encoding::gsm7 => SmsEncoding::Gsm7,
        Encoding::ucs2 => SmsEncoding::Ucs2,
    }
}

/// Resolve the adapter a `routed` message must submit through.
/// `Message.providerId` (stamped by `claim.rs`'s `accepted` branch — since
/// #62, `routing::decide`'s winning route, not `cheapest_active_provider`)
/// names a `Provider` *row*; this looks that row's `key` up and resolves it
/// against `ctx.providers`, the adapters this process actually holds
/// credentials for. `Err` carries a human reason rather than a typed error
/// — the caller folds it into [`ProviderError::Unavailable`]'s own
/// backoff-and-retry handling, since both failure modes here are the same
/// shape as "the provider is broadly unreachable right now": a `Provider`
/// row deleted since routing, or — a real operational case, not just
/// defensive coding — a `Route` naming a provider this particular
/// `dispatch` process has no credentials configured for (a multi-node
/// deployment could split providers across processes; today `dispatch` is
/// a singleton per §7.1, so this specific case keeps retrying until an
/// operator fixes the mismatch, which is the correct, safe behaviour, not
/// a crash).
async fn resolve_provider(
    ctx: &WorkerContext,
    sys: &CoolContext,
    message: &Message,
) -> Result<Arc<dyn SmsProvider>, String> {
    let Some(provider_id) = message.providerId.clone() else {
        return Err("message reached routed with no providerId stamped".to_owned());
    };

    let rows = ctx
        .db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(provider::id().eq(provider_id.clone())))
        .limit(1)
        .run(sys)
        .await
        .map_err(|error| format!("looking up provider {provider_id}: {error}"))?;

    let Some(row) = rows.into_iter().next() else {
        return Err(format!("provider {provider_id} no longer exists"));
    };

    ctx.providers.get(row.key.as_str()).cloned().ok_or_else(|| {
        format!(
            "no adapter configured in this process for provider key {:?} (provider {provider_id})",
            row.key
        )
    })
}

/// Submit one already-`routed` message and write back whichever transition
/// its outcome implies. Errors writing that transition are logged, not
/// propagated — one message's DB write failing must not stall the rest of
/// this tick's batch.
async fn submit_one(ctx: &WorkerContext, sys: &CoolContext, message: Message) {
    // `Message.body` is nullable in the schema (a future retention pass
    // may redact it), but every row this loop ever sees is freshly created
    // and long before its own `expiresAt`, let alone a 90-day retention
    // purge — a `None` here is a genuine anomaly, not a case to paper over
    // with a default.
    let Some(body) = message.body.clone() else {
        warn!(message_id = %message.id, "routed message has no body; failing rather than guessing");
        write_transition(
            ctx,
            sys,
            &message,
            MessageState::failed,
            Some("message body missing at submit time".to_owned()),
            None,
            None,
        )
        .await;
        return;
    };

    let provider = match resolve_provider(ctx, sys, &message).await {
        Ok(provider) => provider,
        Err(reason) => {
            warn!(message_id = %message.id, %reason, "could not resolve a provider adapter for a routed message");
            let (next_state, reason, backoff) =
                classify(&ProviderError::Unavailable { message: reason });
            write_transition(ctx, sys, &message, next_state, Some(reason), backoff, None).await;
            return;
        }
    };

    let req = SubmitRequest {
        to: message.msisdn.clone(),
        sender_id: message.senderIdValue.clone(),
        body,
        encoding: decode_encoding(message.encoding),
        reference: message.id.clone(),
    };

    match provider.submit(&req).await {
        Ok(ack) => {
            write_submitted(
                ctx,
                sys,
                &message,
                &ack.provider_ref,
                ack.provider_ref_alt.as_deref(),
            )
            .await;
        }
        Err(err) => {
            // Known statically, independent of whether the provider ever
            // answered: `req.reference` (== `message.id`) is exactly what
            // was sent as `callbackData` before the network call was
            // attempted. Only worth persisting for the one outcome a later
            // DLR might still need it for — see `write_transition`'s doc.
            let provider_ref_alt =
                matches!(err, ProviderError::Indeterminate { .. }).then_some(message.id.as_str());
            let (next_state, reason, backoff) = classify(&err);
            write_transition(
                ctx,
                sys,
                &message,
                next_state,
                Some(reason),
                backoff,
                provider_ref_alt,
            )
            .await;
        }
    }
}

/// §6.1's [`ProviderError::routing`] gives the general four-way decision;
/// this narrows it to §7.4's actual edges out of `routed` for a
/// single-provider M2 deployment, where "try a different route" and
/// "open the circuit and try a different route" have nowhere else to go
/// yet (M5, #62/#63).
///
/// A backoff transition always targets `queued` regardless of `attempts`
/// — `queued -> failed: max attempts` (§7.4) is enforced exactly once, in
/// [`crate::claim::Claimable::take_lease`]'s own `queued` branch, not
/// duplicated here. The *next* claim attempt is what decides whether
/// `attempts >= maxAttempts` turns this row's next cycle into `failed`
/// instead of another `routed` attempt.
///
/// `ProviderError::Indeterminate` is the one arm that does not fit that
/// pattern at all: it targets `uncertain`, not `queued`, and carries no
/// backoff, because there must be no next attempt. `uncertain` is outside
/// [`crate::claim::Claimable::candidates`]'s state filter
/// (`accepted`/`queued`/`routed`/`undelivered` only, #122), so once a
/// message lands there this loop never picks it up again — the only ways
/// out are a later DLR
/// (`sms_api::dlr`, correlating on `providerMessageRefAlt` since we may
/// never have gotten a `providerMessageRef` to store) or
/// `expire_stale`'s 6h grace (`crates/sms-worker/src/jobs/expire_stale.rs`).
/// This is a deliberate trade: a message that really did fail silently on
/// the provider's side, with no DLR ever coming, sits unresolved for up to
/// 6 hours before `expired` rather than being retried quickly — accepted
/// because the alternative is a real, if less likely, duplicate SMS to a
/// real handset.
fn classify(err: &ProviderError) -> (MessageState, String, Option<Duration>) {
    match err {
        ProviderError::Transient {
            retry_after,
            message: msg,
        } => (
            MessageState::queued,
            format!("transient: {msg}"),
            Some(*retry_after),
        ),
        ProviderError::Unavailable { message: msg } => (
            MessageState::queued,
            format!("provider unavailable: {msg}"),
            Some(UNAVAILABLE_BACKOFF),
        ),
        ProviderError::Permanent { code, message: msg } => (
            MessageState::failed,
            format!("{code}: {msg} (no alternate provider available in this deployment)"),
            None,
        ),
        ProviderError::Rejected { code, message: msg } => {
            (MessageState::failed, format!("{code}: {msg}"), None)
        }
        ProviderError::Unsupported => (
            MessageState::failed,
            "operation not supported by this provider".to_owned(),
            None,
        ),
        ProviderError::Indeterminate { message: msg } => (
            MessageState::uncertain,
            format!("submission outcome unknown, possibly already sent; not retrying: {msg}"),
            None,
        ),
    }
}

/// `routed -> submitted` on a successful [`sms_provider::SmsProvider::submit`].
/// `submittedAt` is stamped by the same trigger that enforces the
/// transition table (§2.10) — never set explicitly here.
async fn write_submitted(
    ctx: &WorkerContext,
    sys: &CoolContext,
    message: &Message,
    provider_ref: &str,
    provider_ref_alt: Option<&str>,
) {
    if let Err(error) = ctx
        .db
        .message()
        .update(message.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::submitted),
            providerMessageRef: Some(Some(provider_ref.to_owned())),
            providerMessageRefAlt: Some(provider_ref_alt.map(ToOwned::to_owned)),
            ..Default::default()
        })
        .if_match(message.version)
        .run(sys)
        .await
    {
        log_write_failure(&message.id, MessageState::submitted, &error);
    }
}

/// `routed -> queued` (backoff, `leaseUntil` set to enforce the delay
/// through the same mechanism [`crate::claim`]'s reclaim uses),
/// `routed -> failed`, or `routed -> uncertain`, per [`classify`]'s
/// outcome.
///
/// `provider_ref_alt`, when given, is stamped onto
/// `Message.providerMessageRefAlt` alongside the transition — used only
/// for the `Indeterminate` -> `uncertain` case. `SubmitRequest::reference`
/// (always `message.id`, see [`submit_one`]) is sent to the provider
/// *before* the network call that might time out, so it's known
/// regardless of whether a response ever comes back — unlike
/// `SubmitAck::provider_ref`/`provider_ref_alt`, which only exist on
/// success. Without recording it here, a message that lands in
/// `uncertain` would have neither `providerMessageRef` nor
/// `providerMessageRefAlt` set, and `sms_api::dlr::ingest_one`'s
/// correlation query (`providerId` + (`providerMessageRef` OR
/// `providerMessageRefAlt`)) would never match a DLR that later echoes
/// this same reference back — see `OrangeCmProvider::submit`'s own doc on
/// `callbackData` always being `req.reference`. Every other transition out
/// of `routed` either retries (no correlation needed yet) or is terminal
/// in a way no later DLR can revisit, so `None` elsewhere is deliberate,
/// not an oversight.
async fn write_transition(
    ctx: &WorkerContext,
    sys: &CoolContext,
    message: &Message,
    next_state: MessageState,
    reason: Option<String>,
    backoff: Option<Duration>,
    provider_ref_alt: Option<&str>,
) {
    let lease_until = backoff.map(|delay| {
        chrono::Utc::now()
            + chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::seconds(30))
    });
    if let Err(error) = ctx
        .db
        .message()
        .update(message.id.clone())
        .set(UpdateMessageInput {
            state: Some(next_state),
            stateReason: Some(reason),
            leaseUntil: Some(lease_until),
            providerMessageRefAlt: provider_ref_alt.map(|r| Some(r.to_owned())),
            ..Default::default()
        })
        .if_match(message.version)
        .run(sys)
        .await
    {
        log_write_failure(&message.id, next_state, &error);
    }
}

/// The only realistic cause of this write failing is a human operator
/// concurrently cancelling the same message — a legitimate race, not a
/// bug, so this logs and lets the tick continue rather than treating it as
/// fatal.
fn log_write_failure(message_id: &str, attempted_state: MessageState, error: &CoolError) {
    warn!(
        message_id,
        attempted_state = ?attempted_state, %error,
        "writing dispatch's own transition failed — likely a concurrent operator action"
    );
}

#[cfg(test)]
mod tests {
    use super::{budget_for, classify, decode_encoding, UNAVAILABLE_BACKOFF};
    use sms_api::schema::{Encoding, MessageState};
    use sms_encoding::SmsEncoding;
    use sms_provider::ProviderError;
    use std::time::Duration;

    #[test]
    fn budget_scales_with_the_tps_ceiling() {
        assert_eq!(budget_for(5.0), 5);
        assert_eq!(
            budget_for(0.5),
            1,
            "a fractional tick must not round to zero"
        );
        assert_eq!(
            budget_for(0.0),
            1,
            "a zero ceiling must still claim something, not stall forever"
        );
    }

    #[test]
    fn encoding_round_trips_through_the_schemas_form() {
        assert_eq!(decode_encoding(Encoding::gsm7), SmsEncoding::Gsm7);
        assert_eq!(decode_encoding(Encoding::ucs2), SmsEncoding::Ucs2);
    }

    #[test]
    fn transient_backs_off_and_stays_queued() {
        let (state, _, backoff) = classify(&ProviderError::Transient {
            retry_after: Duration::from_secs(5),
            message: "rate limited".to_owned(),
        });
        assert_eq!(state, MessageState::queued);
        assert_eq!(backoff, Some(Duration::from_secs(5)));
    }

    #[test]
    fn unavailable_backs_off_with_a_fixed_delay_and_stays_queued() {
        let (state, _, backoff) = classify(&ProviderError::Unavailable {
            message: "connection refused".to_owned(),
        });
        assert_eq!(state, MessageState::queued);
        assert_eq!(backoff, Some(UNAVAILABLE_BACKOFF));
    }

    #[test]
    fn permanent_fails_outright_in_a_single_provider_deployment() {
        let (state, reason, backoff) = classify(&ProviderError::Permanent {
            code: "SENDER_ID_NOT_APPROVED".to_owned(),
            message: "sender id not approved".to_owned(),
        });
        assert_eq!(state, MessageState::failed);
        assert_eq!(backoff, None);
        assert!(reason.contains("no alternate provider"));
    }

    #[test]
    fn rejected_and_unsupported_both_fail_outright() {
        let (rejected_state, _, rejected_backoff) = classify(&ProviderError::Rejected {
            code: "INVALID_DESTINATION".to_owned(),
            message: "bad number".to_owned(),
        });
        assert_eq!(rejected_state, MessageState::failed);
        assert_eq!(rejected_backoff, None);

        let (unsupported_state, _, unsupported_backoff) = classify(&ProviderError::Unsupported);
        assert_eq!(unsupported_state, MessageState::failed);
        assert_eq!(unsupported_backoff, None);
    }

    /// The whole point of this ticket: an indeterminate outcome must land
    /// in `uncertain`, not `queued` (which `claim.rs`'s `candidates()`
    /// would pick straight back up and resubmit) and not `failed` (which
    /// would discard a message that might still resolve via a late DLR).
    /// No backoff either — `uncertain` isn't in the claimable set at all,
    /// so a `leaseUntil` on it would be meaningless.
    #[test]
    fn indeterminate_lands_in_uncertain_with_no_backoff_and_no_retry() {
        let (state, reason, backoff) = classify(&ProviderError::Indeterminate {
            message: "read timeout after the request was sent".to_owned(),
        });
        assert_eq!(state, MessageState::uncertain);
        assert_eq!(backoff, None);
        assert!(reason.contains("not retrying"));
    }
}
