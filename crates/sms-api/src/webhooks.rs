//! #38 — subscribers that turn `@@emit`'d model events into `WebhookAttempt`
//! rows. See §8 of the design doc for the full design; this module doc
//! covers the two things that shaped its code and aren't obvious from
//! reading it cold.
//!
//! # The hard constraint every function here is written against
//!
//! `@@emit` delivery (`cratestack_event_outbox` → `CoolEventBus::emit`,
//! §8.2) is **synchronous, blocks the mutation that triggered it, and is
//! not panic-isolated**. A subscriber that blocks or panics breaks
//! `sendMessage`, `dlr::ingest`, or whatever else touched an emitting
//! model. So every subscriber here does exactly one thing — read the
//! event, look up matching `WebhookEndpoint` rows, insert `WebhookAttempt`
//! rows — and [`register_subscribers`] wraps each one in `tokio::spawn` so
//! a bug becomes a logged `Err` (a `JoinError`) rather than an unwind
//! through the mutation's own call stack. No HTTP call, no retry, no
//! branching beyond "does this state map to a catalogued event type" and
//! "which endpoints subscribe to it" — all real delivery is the `hooks`
//! role's job (M3 #40), not this module's.
//!
//! # Resolving #38 vs #39: if subscribers already insert attempts
//! synchronously, what does `drain` (#39) drain?
//!
//! `db.events().on_message_created(...)`/`on_message_updated(...)` each
//! register against a `Cratestack`/`SqlxRuntime` instance's own **in-process**
//! `CoolEventBus` (`cratestack_sqlx::descriptor::SqlxRuntime::subscribe`,
//! read directly in the vendored source, not assumed) — registration
//! never crosses a process boundary, and *every* `@@emit`-annotated
//! mutation triggers an automatic drain of its own process's runtime
//! immediately after commit (`cratestack-sqlx`'s `create.rs`/`update.rs`:
//! `let _ = self.runtime.drain_event_outbox().await;`, unconditional, no
//! `db.events().drain()` call required to trigger it).
//!
//! That has a sharp edge, and it is the actual answer to the question
//! above: **`CoolEventBus::emit` returns `Ok(())` for a topic with zero
//! registered handlers** (`cratestack-core/src/events/bus.rs`: an empty
//! handler `Vec`, an empty `for` loop, `Ok(())`) — not an error, not a
//! skip flagged anywhere. So a process that writes to an emitting model
//! (`Message`, in this milestone's scope) *without* having called
//! [`register_subscribers`] on its own `Cratestack` instance first does
//! not "leave the row for `drain` to pick up later." Its own automatic
//! post-commit drain call marks the row `delivered_at = NOW()`
//! immediately, having done nothing, and `drain_event_outbox`'s own
//! `SELECT ... WHERE delivered_at IS NULL` never sees it again. The event
//! is not stalled; it is **lost, silently, the moment the write
//! commits** — a worse failure mode than the one #39 names.
//!
//! That makes [`register_subscribers`] mandatory plumbing in **every**
//! process whose own `Cratestack` instance ever writes to an emitting
//! model, not optional wiring for wherever the `drain` role happens to be
//! scheduled: `app/sms-gateway` (`sendMessage`, `dlr::ingest`, both write
//! `Message`) and `app/sms-worker` (`dispatch` writes `Message`;
//! `jobs::expire_stale` writes it too). `app/sms-worker` registers once in
//! `main`, before any role task is spawned, against the one `Cratestack`
//! every role's `WorkerContext` clones — `Cratestack`/`SqlxRuntime`/
//! `CoolEventBus` all derive `Clone` over `Arc`-backed state, so a clone
//! shares the same live handler registry, not a copy of it. One
//! registration call covers every role that process runs, including ones
//! that never touch an emitting model themselves (`hooks`, `jobs`) — those
//! registrations just sit unused, which costs nothing.
//!
//! Given all of that, what `crate::drain`'s `drain` role (#39, in
//! `crates/sms-worker`) actually adds on top of every writer's own
//! automatic post-commit drain is exactly the one thing no writer path
//! gives you: a handler that failed on its first attempt (a transient
//! error creating the `WebhookAttempt` row, say) is left
//! `delivered_at IS NULL` with `attempts`/`last_error` recorded by
//! `drain_event_outbox` itself, and **nothing retries it until the next
//! drain** — which, without a write-independent trigger, only happens
//! whenever the next mutation on *any* emitting model happens to occur.
//! `drain`'s periodic call, unconditional on any mutation happening at
//! all, is that trigger — the literal fix for "the framework runs no
//! background drain worker" (§8.2).
//!
//! # `WebhookEndpoint`'s missing `hasRole('system')` — the eighth instance
//!
//! [`enqueue_message_webhook_attempts`] reads `WebhookEndpoint` under a
//! `system` context to find which endpoints subscribe to a given event
//! type. Before this change `WebhookEndpoint`'s `@@allow("read", ...)`
//! clause was `auth().kind == "user"` only — the same shape `AGENTS.md`'s
//! "Invariants that fail the build rather than production" section has
//! recorded seven times before (`App`, `AppClient`,
//! `SenderIdRegistration`, `OperatorPrefixRule`, `Provider`, `Job`,
//! `DeliveryReceipt`): a missing `hasRole('system')` clause doesn't error,
//! it silently filters a system context's read down to an empty array.
//! `schema.cstack` now adds it (policy-only — no DDL consequence, so
//! `0001_init` was not regenerated, per `AGENTS.md`'s own standing rule);
//! `crates/sms-api/tests/system_context_golden_list_live_postgres.rs`
//! moves `WebhookEndpoint` into `SYSTEM_READABLE_MODELS` to match. Found
//! here, by this PR's own live suite, before merge — not live in
//! production, which is the entire point of that golden test existing.
//!
//! # The stored payload is `data` only, not a full envelope
//!
//! §8.4's example webhook body has an outer envelope (`id`, `type`,
//! `occurredAt`) wrapping a `data` object. This module stores only the
//! `data` object in `WebhookAttempt.payload` — the outer `id` in that
//! example is not `Message.id` (the example's own `data.messageId` is a
//! *different* id from the top-level `id`), and the only value that
//! naturally supplies a distinct, stable, per-attempt id is
//! `WebhookAttempt.id` itself, which does not exist yet at the moment
//! this subscriber runs (it's the row this subscriber is in the middle of
//! creating). Building the final signed envelope — `id` from the
//! `WebhookAttempt` row, `type` from its `eventType` column, `occurredAt`
//! from its `createdAt` — is naturally the `hooks` role's job (#40) at
//! the moment it actually POSTs, alongside signing (#41), not this
//! module's.

use chrono::Utc;
use cratestack::{CoolContext, CoolError, FilterExpr};
use tracing::error;

use crate::auth::{Principal, PrincipalKind};
use crate::errors::UNIQUE_VIOLATION;
use crate::schema::{
    events::{MessageCreatedEvent, MessageUpdatedEvent},
    webhook_endpoint, Cratestack, CreateWebhookAttemptInput, Message, MessageState,
};

/// The `system` context every subscriber in this module reads/writes
/// under. No real caller's token ever carries this identity — see
/// `Principal::into_context`'s own doc for why setting only `kind` or only
/// `role` denies every write instead of granting one.
fn sys() -> CoolContext {
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
/// `CoolEventBus`. See this module's own doc for why calling this exactly
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
async fn panic_isolated<F>(fut: F) -> Result<(), CoolError>
where
    F: std::future::Future<Output = Result<(), CoolError>> + Send + 'static,
{
    match tokio::spawn(fut).await {
        Ok(result) => result,
        Err(join_error) => {
            error!(
                %join_error,
                "webhook subscriber panicked; event left undelivered for retry"
            );
            Err(CoolError::Internal(format!(
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
pub async fn enqueue_message_webhook_attempts(
    db: &Cratestack,
    source_event_id: cratestack::uuid::Uuid,
    message: &Message,
) -> Result<(), CoolError> {
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
    /// `CoolEventBus::emit` → `SqlxRuntime::drain_event_outbox` → the
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
            Err(cratestack::CoolError::Internal(
                "ordinary failure".to_owned(),
            ))
        })
        .await;
        assert!(matches!(result, Err(cratestack::CoolError::Internal(_))));
    }
}
