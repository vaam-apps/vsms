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

use std::time::Duration;

use cratestack::{CoolContext, CoolError};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{Encoding, Message, MessageState, UpdateMessageInput};
use sms_encoding::SmsEncoding;
use sms_provider::{ProviderError, SubmitRequest};
use tracing::{error, warn};

use crate::claim::claim_batch;
use crate::WorkerContext;

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
    let budget = budget_for(ctx.provider.capabilities().tps_ceiling);
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

fn decode_encoding(encoding: Encoding) -> SmsEncoding {
    match encoding {
        Encoding::gsm7 => SmsEncoding::Gsm7,
        Encoding::ucs2 => SmsEncoding::Ucs2,
    }
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
        )
        .await;
        return;
    };

    let req = SubmitRequest {
        to: message.msisdn.clone(),
        sender_id: message.senderIdValue.clone(),
        body,
        encoding: decode_encoding(message.encoding),
        reference: message.id.clone(),
    };

    match ctx.provider.submit(&req).await {
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
            let (next_state, reason, backoff) = classify(&err);
            write_transition(ctx, sys, &message, next_state, Some(reason), backoff).await;
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
/// through the same mechanism [`crate::claim`]'s reclaim uses) or
/// `routed -> failed`, per [`classify`]'s outcome.
async fn write_transition(
    ctx: &WorkerContext,
    sys: &CoolContext,
    message: &Message,
    next_state: MessageState,
    reason: Option<String>,
    backoff: Option<Duration>,
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
}
