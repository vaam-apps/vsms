#![doc = include_str!("webhooks.md")]

use chrono::Utc;
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use tracing::error;

use crate::auth::{Principal, PrincipalKind};
use crate::errors::UNIQUE_VIOLATION;
use crate::schema::{
    Cratestack, CreateWebhookAttemptInput, Message, MessageState,
    events::{MessageCreatedEvent, MessageUpdatedEvent},
    webhook_endpoint,
};

/// The `system` context every subscriber in this module reads/writes
/// under. No real caller's token ever carries this identity — see
/// `Principal::into_context`'s own doc for why setting only `kind` or only
/// `role` denies every write instead of granting one.
fn sys() -> CratestackContext {
    Principal {
        sub: "sms-api:webhooks".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// §8.4's event catalogue, restricted to the `Message` states it actually
/// names. `None` for `queued`/`routed` (internal routing machinery, never
/// in the catalogue) and for `undelivered`/`rejected` (real, reachable
/// states — `undelivered`'s own gap is `AGENTS.md`'s documented #122 — but
/// likewise absent from §8.4's own list; widening the catalogue to cover
/// them is a product decision for whoever picks that up next, not
/// something to invent here).
///
/// **`accepted` is reachable only from `Message.created`, never from an
/// `updated` event.** It's the schema's own `@default('accepted')` — the
/// row's state the instant it's created — and `message_state_transitions`
/// (§2.10) lists it only as a `from_state`, never a `to_state`: nothing
/// transitions *into* `accepted`, ever. A caller that maps this function's
/// `Some("message.accepted")` return value onto `on_message_updated` alone
/// would advertise an event type that can structurally never fire — found
/// by Lightbridge's review of this PR, confirmed against
/// `message_state_transitions` before fixing: [`register_subscribers`]
/// wires up `on_message_created` for exactly this reason, alongside
/// `on_message_updated`, both driving the same
/// [`enqueue_message_webhook_attempts`].
#[must_use]
pub fn message_event_type(state: MessageState) -> Option<&'static str> {
    match state {
        MessageState::accepted => Some("message.accepted"),
        MessageState::submitted => Some("message.submitted"),
        MessageState::delivered => Some("message.delivered"),
        MessageState::failed => Some("message.failed"),
        MessageState::expired => Some("message.expired"),
        MessageState::uncertain => Some("message.uncertain"),
        MessageState::cancelled => Some("message.cancelled"),
        MessageState::queued
        | MessageState::routed
        | MessageState::undelivered
        | MessageState::rejected => None,
    }
}

/// Registers every subscriber this milestone builds against `db`'s own
/// `CratestackEventBus`. See this module's own doc for why calling this exactly
/// once per process — not once per role, not gated on which roles a
/// `sms-worker` process runs — is the actual correctness requirement, not
/// a style preference.
///
/// **Both `Message.created` and `Message.updated` are wired up, deliberately
/// together, in this one function.** `Message.created` is what makes
/// `message.accepted` reachable at all — see [`message_event_type`]'s own
/// doc for why `updated` alone can never produce it — and the fix for that
/// finding is here, not split across two call sites, so nothing can ever
/// register one without the other. Both drive the exact same
/// [`enqueue_message_webhook_attempts`]: the function only cares that it
/// received a `Message` row and an `event_id`, never which operation
/// produced them, so `created`/`updated` share one implementation rather
/// than duplicating the endpoint-lookup-and-insert logic per event kind.
///
/// `OptOut.created`, `Provider.updated` (→ `provider.degraded`/
/// `provider.recovered`) and a hypothetical `SenderIdRegistration` emit (→
/// `sender_id.approved`/`sender_id.rejected`, which would first need
/// `@@emit` added to a model that doesn't have it today) are explicitly out
/// of scope: `OptOut` has no `appId`, and `WebhookEndpoint.appId` is what
/// every match in this module keys on — which endpoints a global,
/// cross-app opt-out should notify is a product decision this PR doesn't
/// make, not an oversight. `Provider`/`SenderIdRegistration` need a similar
/// real decision (what counts as "degraded", whether `SenderIdRegistration`
/// should emit at all). Follows the same scoping precedent as #35's single
/// `expire_stale` job kind: build the one path this milestone's own state
/// machine already makes unambiguous, name the rest as deliberately cut.
pub fn register_subscribers(db: &Cratestack) {
    let created_db = db.clone();
    db.events()
        .on_message_created(move |event: MessageCreatedEvent| {
            let db = created_db.clone();
            panic_isolated(async move {
                enqueue_message_webhook_attempts(&db, event.event_id, &event.data).await
            })
        });

    let updated_db = db.clone();
    db.events()
        .on_message_updated(move |event: MessageUpdatedEvent| {
            let db = updated_db.clone();
            panic_isolated(async move {
                enqueue_message_webhook_attempts(&db, event.event_id, &event.data).await
            })
        });
}

/// Wraps `fut` in `tokio::spawn` so a panic inside it surfaces as a
/// `JoinError` here, not an unwind through whatever mutation's post-commit
/// drain called this handler. §8.2: "handlers are not panic-isolated — no
/// `catch_unwind` in the framework itself." This is the boundary #38 says
/// has to exist, and this is where it lives.
async fn panic_isolated<F>(fut: F) -> Result<(), CratestackError>
where
    F: std::future::Future<Output = Result<(), CratestackError>> + Send + 'static,
{
    match tokio::spawn(fut).await {
        Ok(result) => result,
        Err(join_error) => {
            error!(
                %join_error,
                "webhook subscriber panicked; event left undelivered for retry"
            );
            Err(CratestackError::Internal(format!(
                "webhook subscriber panicked: {join_error}"
            )))
        }
    }
}

/// The actual `Message.created`/`Message.updated` subscriber body, shared
/// by both closures in [`register_subscribers`] and factored out here so
/// it's directly callable from tests without going through the event bus
/// at all.
///
/// No-ops (returns `Ok(())` without touching the database) when
/// [`message_event_type`] doesn't map `message.state` to a catalogued
/// event — most `Message.updated` events are internal routing hops
/// (`accepted -> queued -> routed`) nobody outside this system should ever
/// hear about. Every `Message.created` call, by contrast, always maps —
/// `state` is unconditionally `accepted` the instant a row is created,
/// per the schema's own `@default('accepted')`.
///
/// **Also no-ops on a purged message — #67's own guard, found by the
/// coordinator's review of that PR, not by this module's own author.**
/// `Message` carries `@@emit(created, updated)`, and
/// `backends/crates/sms-worker/src/jobs/purge_retention.rs` writes through a real
/// `.update()`, which means every purge fires this exact subscriber: four
/// of the job's five terminal candidate states
/// (`delivered`/`failed`/`expired`/`cancelled` — every one but `rejected`)
/// map to a catalogued event per [`message_event_type`]. Without this
/// guard, purging a 90-day-old message would enqueue — and `hooks` would
/// then sign and POST — a live webhook to the customer's endpoint
/// carrying the placeholder `to: "purged-msisdn"` and a `clientRef` that
/// is already `None`, reporting on a message the endpoint was already
/// told about three months earlier. `webhook_attempts_dedupe`
/// (`endpoint_id`, `aggregate_id`, `event_type`) happens to swallow the
/// common case where an attempt for this exact event already exists from
/// the original delivery — but that is a coincidence of an unrelated
/// unique index, not a guard against emitting an event we never meant to
/// send, and it does not save an endpoint registered *after* the
/// original event fired, which has no prior row to collide with. `body`
/// is deliberately not part of this check: `body` can in principle be
/// null for reasons unrelated to a purge (see §2.5's own note on the
/// #183 redact-at-terminal-state idea, built and rejected) — `purgedAt`
/// is the one field whose meaning is exactly "this row must never be
/// reported on again."
pub async fn enqueue_message_webhook_attempts(
    db: &Cratestack,
    source_event_id: cratestack::uuid::Uuid,
    message: &Message,
) -> Result<(), CratestackError> {
    if message.purgedAt.is_some() {
        return Ok(());
    }

    let Some(event_type) = message_event_type(message.state) else {
        return Ok(());
    };

    let sys = sys();
    let endpoints = db
        .webhook_endpoint()
        .find_many()
        .where_expr(
            FilterExpr::from(webhook_endpoint::appId().eq(message.appId.clone()))
                .and(webhook_endpoint::active().is_true())
                .and(webhook_endpoint::eventTypes().contains(sms_core::needle(event_type))),
        )
        .run(&sys)
        .await?;

    for endpoint in endpoints {
        let payload = message_payload(message, endpoint.maskRecipient);
        match db
            .webhook_attempt()
            .create(CreateWebhookAttemptInput {
                endpointId: endpoint.id.clone(),
                sourceEventId: source_event_id,
                aggregateId: message.id.clone(),
                eventType: event_type.to_owned(),
                payload,
                leaseOwner: None,
                leaseUntil: None,
                nextAttemptAt: Some(Utc::now()),
                lastStatusCode: None,
                lastError: None,
                lastAttemptAt: None,
                deliveredAt: None,
            })
            .run(&sys)
            .await
        {
            Ok(_) => {}
            // webhook_attempts_dedupe (endpoint_id, aggregate_id,
            // event_type): already enqueued, by an earlier drain of this
            // same event or by an earlier update to the same message that
            // happened to derive the same event type. Not an error — see
            // §8.3's own reasoning for why aggregate+type, not
            // source_event_id, is the dedupe key.
            Err(error) if error.db_sqlstate() == Some(UNIQUE_VIOLATION) => {}
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

/// The `data` object §8.4 shows nested under a webhook's outer envelope —
/// see this module's own doc for why the envelope itself isn't built here.
/// `to` is masked via `sms_msisdn::Msisdn::masked` when `mask_recipient`
/// is set, computed once per matching endpoint since two endpoints on the
/// same message may disagree on `maskRecipient`.
fn message_payload(message: &Message, mask_recipient: bool) -> String {
    let to = sms_msisdn::Msisdn::parse(&message.msisdn).map_or_else(
        // An unparseable stored msisdn shouldn't block the webhook that
        // reports on it — fall back to the raw stored value rather than
        // failing the whole subscriber over a formatting concern.
        |_| message.msisdn.clone(),
        |parsed| {
            if mask_recipient {
                parsed.masked()
            } else {
                parsed.as_e164().to_owned()
            }
        },
    );

    let data = serde_json::json!({
        "messageId": message.id,
        "appId": message.appId,
        "clientRef": message.clientRef,
        "to": to,
        "state": message.state,
        "operator": message.operator,
        "segments": message.segments,
        "costXaf": message.costXaf.to_string(),
    });
    data.to_string()
}

#[cfg(test)]
mod tests {
    use super::message_event_type;
    use crate::schema::MessageState;

    #[test]
    fn catalogued_states_map_to_the_documented_event_type() {
        assert_eq!(
            message_event_type(MessageState::accepted),
            Some("message.accepted")
        );
        assert_eq!(
            message_event_type(MessageState::submitted),
            Some("message.submitted")
        );
        assert_eq!(
            message_event_type(MessageState::delivered),
            Some("message.delivered")
        );
        assert_eq!(
            message_event_type(MessageState::failed),
            Some("message.failed")
        );
        assert_eq!(
            message_event_type(MessageState::expired),
            Some("message.expired")
        );
        assert_eq!(
            message_event_type(MessageState::uncertain),
            Some("message.uncertain")
        );
        assert_eq!(
            message_event_type(MessageState::cancelled),
            Some("message.cancelled")
        );
    }

    #[test]
    fn internal_routing_and_uncatalogued_states_produce_no_event() {
        assert_eq!(message_event_type(MessageState::queued), None);
        assert_eq!(message_event_type(MessageState::routed), None);
        assert_eq!(message_event_type(MessageState::undelivered), None);
        assert_eq!(message_event_type(MessageState::rejected), None);
    }

    /// The property #38 exists to guarantee, proven directly and without a
    /// database: a subscriber body that panics must not unwind into
    /// whatever called it. Without `tokio::spawn`'s task boundary, a panic
    /// inside a future awaited in-line propagates exactly like a
    /// synchronous panic — which, chained through
    /// `CratestackEventBus::emit` → `SqlxRuntime::drain_event_outbox` → the
    /// `@@emit` mutation's own post-commit call, would abort whatever
    /// procedure (`sendMessage`, `dlr::ingest`) triggered it.
    #[tokio::test]
    async fn a_panicking_subscriber_body_becomes_an_err_not_an_unwind() {
        let result = super::panic_isolated(async {
            panic!("simulated subscriber bug");
            #[allow(unreachable_code)]
            Ok(())
        })
        .await;
        assert!(
            result.is_err(),
            "a panicking subscriber must surface as Err, not unwind"
        );
    }

    /// The non-panicking path still returns whatever the wrapped future
    /// returned — `panic_isolated` only ever changes behaviour on an
    /// actual panic, never on an ordinary `Err`.
    #[tokio::test]
    async fn a_non_panicking_err_passes_through_unchanged() {
        let result = super::panic_isolated(async {
            Err(cratestack::CratestackError::Internal(
                "ordinary failure".to_owned(),
            ))
        })
        .await;
        assert!(matches!(
            result,
            Err(cratestack::CratestackError::Internal(_))
        ));
    }
}
