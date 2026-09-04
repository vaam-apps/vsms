#![doc = include_str!("dispatch.md")]

use std::sync::Arc;
use std::time::Duration;

use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::auth::system_context;
use sms_api::map_database_error;
use sms_api::schema::{
    Encoding, Message, MessageState, Provider, UpdateMessageInput, UpdateProviderInput, provider,
};
use sms_encoding::SmsEncoding;
use sms_provider::{ProviderError, RoutingConsequence, SmsProvider, SubmitRequest};
use tracing::{error, info, warn};

use crate::breaker::{self, BreakerPolicy};
use crate::claim::claim_batch;
use crate::{ProviderRegistry, WorkerContext};

/// How often this loop polls for claimable messages.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// `ProviderError::Unavailable` carries no `retry_after` of its own (§6.1:
/// it means the provider broadly, not this one message) — this crate picks
/// a conservative fixed backoff rather than retrying immediately. Also the
/// fallback backoff once #63's own failover is exhausted and there is
/// nothing left to do but retry the same (still-unavailable) provider.
const UNAVAILABLE_BACKOFF: Duration = Duration::from_secs(30);

/// §6.3, verbatim: "Failover capped at two hops — beyond that you're not
/// routing, you're spraying." Enforced in [`attempt_failover`] against
/// `Message.excludedRouteIds`'s own length, not a separately tracked
/// counter — see that field's doc in `schema.cstack`.
const MAX_FAILOVER_HOPS: usize = 2;

/// §6.3, verbatim: "five consecutive `Unavailable` opens it for 60s."
/// Unlike `hooks::ENDPOINT_BREAKER` (webhook endpoints, 20 failures /
/// 15 minutes) — different subsystem, different spec'd numbers, same
/// *shape* of breaker; the shared decision itself lives in
/// `crate::breaker`, see that module's own doc for why the numbers stay
/// here rather than there.
const PROVIDER_BREAKER: BreakerPolicy = BreakerPolicy {
    failure_threshold: 5,
    open_duration: chrono::Duration::seconds(60),
};

/// The `system` context this role does all its work under — see
/// [`sms_api::auth::system_context`] for the shared constructor and the
/// invariant it documents.
fn sys(worker: &str) -> CratestackContext {
    system_context(format!("sms-worker:dispatch:{worker}"))
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
pub async fn tick(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    worker: &str,
) -> Result<(), CratestackError> {
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
/// filtering, out of scope for #62/#63 (see `backends/crates/sms-routing`'s own
/// module doc).
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

/// Resolve the adapter a `routed` message must submit through, and the
/// `Provider` row it came from — #63 needs the row itself, not just the
/// adapter, to read/write `consecutiveFailures`/`circuitOpenUntil`/
/// `version` for the circuit breaker (`record_provider_failure`/
/// `reset_provider_failures`, below). `Message.providerId` (stamped by
/// `claim.rs`'s `accepted` branch or by [`attempt_failover`]'s own reroute)
/// names a `Provider` *row*; this looks that row's `key` up and resolves it
/// against `ctx.providers`, the adapters this process actually holds
/// credentials for. `Err` carries a human reason rather than a typed error
/// — the caller folds it into the same `Unavailable`-shaped handling as a
/// real submit failure ([`handle_submit_error`]), since both failure modes
/// here are the same shape as "the provider is broadly unreachable right
/// now": a `Provider` row deleted since routing, or — a real operational
/// case, not just defensive coding — a `Route` naming a provider this
/// particular `dispatch` process has no credentials configured for (a
/// multi-node deployment could split providers across processes; today
/// `dispatch` is a singleton per §7.1, so this specific case keeps
/// retrying until an operator fixes the mismatch, which is the correct,
/// safe behaviour, not a crash).
async fn resolve_provider(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    message: &Message,
) -> Result<(Provider, Arc<dyn SmsProvider>), String> {
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

    let adapter = ctx
        .providers
        .get(row.key.as_str())
        .cloned()
        .ok_or_else(|| {
            format!(
                "no adapter configured in this process for provider key {:?} (provider {provider_id})",
                row.key
            )
        })?;

    Ok((row, adapter))
}

/// Submit one already-`routed` message and write back whichever transition
/// its outcome implies. Errors writing that transition are logged, not
/// propagated — one message's DB write failing must not stall the rest of
/// this tick's batch.
async fn submit_one(ctx: &WorkerContext, sys: &CratestackContext, message: Message) {
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

    let (provider_row, provider) = match resolve_provider(ctx, sys, &message).await {
        Ok(pair) => pair,
        Err(reason) => {
            warn!(message_id = %message.id, %reason, "could not resolve a provider adapter for a routed message");
            handle_submit_error(
                ctx,
                sys,
                &message,
                None,
                &ProviderError::Unavailable { message: reason },
            )
            .await;
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

    // #70: in-flight for exactly the span of the provider call — not the
    // whole of `submit_one`, which also does DB work before and after that
    // this gauge has no business counting. A plain inc-before/dec-after
    // pair, not a `Drop` guard: the only way to skip the `dec()` below is
    // this future being dropped mid-await (a shutdown racing an in-flight
    // submit, in `run_singleton`'s own `tokio::select!`), and a dropped
    // task means this process is exiting imminently anyway — the gauge
    // dies with the process's own `/metrics` the moment it does, so a
    // guard's only advantage here is covering a window that doesn't
    // outlive the process either way.
    //
    // #62 changed what `provider` is: it is now the adapter resolved for
    // *this* message's route, not the process's single hardcoded one, so
    // the label is per-route rather than per-process. That is what makes
    // "unexpected concurrent submits per provider" mean anything once a
    // second provider exists.
    let provider_key = provider.key();
    sms_metrics::DISPATCH_IN_FLIGHT_SUBMITS
        .with_label_values(&[provider_key])
        .inc();
    let submit_result = provider.submit(&req).await;
    sms_metrics::DISPATCH_IN_FLIGHT_SUBMITS
        .with_label_values(&[provider_key])
        .dec();

    match submit_result {
        Ok(ack) => {
            // #71: the second correlation event in the chain — see this
            // module's own doc, and `procedures.rs`'s for the first. No
            // `cratestack_request_id` here: this loop runs under `sys(...)`
            // (an internal `system` context this crate mints itself, never
            // derived from the HTTP request that originally created the
            // message), so there is no request-scoped id to carry forward
            // — `message_id` is the join key across this process boundary,
            // same as the DLR side.
            info!(
                message_id = %message.id,
                provider = provider_key,
                provider_ref = %ack.provider_ref,
                "message submitted"
            );
            write_submitted(
                ctx,
                sys,
                &message,
                &ack.provider_ref,
                ack.provider_ref_alt.as_deref(),
            )
            .await;
            // #63: a successful submit is what actually happened here —
            // this provider is not the one currently misbehaving, and any
            // streak of prior failures it was accumulating is stale.
            reset_provider_failures(ctx, sys, &provider_row).await;
        }
        Err(err) => {
            handle_submit_error(ctx, sys, &message, Some(&provider_row), &err).await;
        }
    }
}

/// #63's own compiler-checked mapping — driven by [`ProviderError::routing`]
/// (`backends/crates/sms-provider/src/error.rs`), not a second, hand-derived
/// four-way decision duplicating it. Exactly which [`RoutingConsequence`]
/// triggers a failover attempt and which is terminal-for-this-message:
///
/// | `RoutingConsequence` | error | what happens |
/// |---|---|---|
/// | `RetryThisProvider { after }` | `Transient` | same-provider backoff, `routed -> queued`; no failover |
/// | `TryNextRoute` | `Permanent` | [`attempt_failover`]; exhausted -> `routed -> failed`, "no alternate route available" |
/// | `OpenCircuitAndTryNextRoute` | `Unavailable` | [`record_provider_failure`], then [`attempt_failover`]; exhausted -> same-provider backoff (the M2 single-provider degradation, still correct once no alternate exists) |
/// | `FailMessage` | `Rejected`/`Unsupported` | `routed -> failed`; no failover — retrying anywhere fails identically |
/// | `HoldIndeterminate` | `Indeterminate` | `routed -> uncertain`; no failover, no retry, unchanged from before #63 — see [`terminal_outcome`]'s own doc |
///
/// `provider_row` is `None` only when [`resolve_provider`] itself failed
/// before ever reaching a real `Provider` row (no row to record a
/// circuit-breaker failure against) — [`submit_one`] synthesises that case
/// as `Unavailable`, so it still reaches `OpenCircuitAndTryNextRoute` and
/// still attempts failover, just without touching any provider's own
/// bookkeeping (there is nothing to touch).
///
/// A [`CratestackError`] from [`attempt_failover`]'s own routing query — not a
/// `PreconditionFailed` on the *reroute* write, which `attempt_failover`
/// already logs and treats as "someone else already moved this row" — is
/// logged and nothing is written: the message is still safely `routed`
/// with its lease intact, so `claim.rs`'s own crash-reclaim path picks it
/// back up once the lease expires, the same fallback this file already
/// relies on for every other failure it only logs.
async fn handle_submit_error(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    message: &Message,
    provider_row: Option<&Provider>,
    err: &ProviderError,
) {
    let consequence = err.routing();

    if matches!(consequence, RoutingConsequence::OpenCircuitAndTryNextRoute)
        && let Some(row) = provider_row
    {
        record_provider_failure(ctx, sys, row).await;
    }

    let should_attempt_failover = matches!(
        consequence,
        RoutingConsequence::TryNextRoute | RoutingConsequence::OpenCircuitAndTryNextRoute
    );
    if should_attempt_failover {
        match attempt_failover(ctx, sys, message, &err.to_string()).await {
            Ok(FailoverOutcome::Rerouted) => return,
            Ok(FailoverOutcome::Exhausted) => {} // fall through to the terminal outcome below
            Err(error) => {
                warn!(
                    message_id = %message.id, %error,
                    "failover routing query failed; leaving routed for lease-expiry reclaim"
                );
                return;
            }
        }
    }

    let (next_state, reason, backoff) = terminal_outcome(err);
    // Known statically, independent of whether the provider ever answered:
    // `req.reference` (== `message.id`) is exactly what was sent as
    // `callbackData` before the network call was attempted. Only worth
    // persisting for the one outcome a later DLR might still need it for
    // — see `write_transition`'s doc.
    let provider_ref_alt =
        matches!(err, ProviderError::Indeterminate { .. }).then_some(message.id.as_str());
    write_transition(
        ctx,
        sys,
        message,
        next_state,
        Some(reason),
        backoff,
        provider_ref_alt,
    )
    .await;
}

/// The terminal-for-this-message outcome once failover has nothing left to
/// offer — either #63's own two-hop cap (`MAX_FAILOVER_HOPS`) was reached,
/// or [`attempt_failover`] found no eligible alternate route at all. Also
/// the *only* outcome for the three [`RoutingConsequence`] variants
/// failover was never going to help with in the first place
/// (`RetryThisProvider`/`FailMessage`/`HoldIndeterminate`) — see
/// [`handle_submit_error`]'s own table for the full mapping.
///
/// Pure and synchronous on purpose — no I/O, no failover decision, just
/// "given this error, and failover already exhausted or never applicable,
/// what does `Message` do next" — so this mapping stays directly
/// unit-testable without a database, the same discipline this file has
/// followed since #33 (this function is `classify`'s own #33-era name,
/// carrying the identical five non-failover-eligible cases verbatim; only
/// `Permanent`'s own reason text changed, from "no alternate provider
/// available in this deployment" to "no alternate route available" — true
/// whether that's because this is still a single-provider deployment or
/// because #63's own two-hop cap was actually reached).
fn terminal_outcome(err: &ProviderError) -> (MessageState, String, Option<Duration>) {
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
            format!("{code}: {msg} (no alternate route available)"),
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

/// What [`attempt_failover`] found.
#[derive(Debug, PartialEq, Eq)]
enum FailoverOutcome {
    /// A new `providerId`/`routeId` was stamped and the row moved back to
    /// `queued` — already written; the caller has nothing further to do.
    Rerouted,
    /// Either the two-hop cap (`MAX_FAILOVER_HOPS`) was already reached, or
    /// `sms_routing::select_route` found no eligible alternate route. The
    /// caller must still write *some* transition — see
    /// [`handle_submit_error`].
    Exhausted,
}

/// #63's own failover mechanism: "give me the next route after this one
/// failed", implemented per `sms_routing::select_route`'s own doc as
/// "call `select_route` again with the failed route's id added to
/// `exclude`" — not a hand-rolled walk of `Route.failoverRouteId` itself
/// (that field is carried through `Winner` for an operator's own
/// explanation trail, never read by this function or by the pure engine —
/// see `backends/crates/sms-routing/src/types.rs`'s own doc on `RouteRow::
/// failover_route_id`).
///
/// `Message.excludedRouteIds` (#63, `schema.cstack`) is this message's own
/// accumulated exclude set — every route it has already been rerouted away
/// from, sentinel-packed (`sms_core::pack`/`unpack`). Necessary, not just
/// convenient: `RoutingConsequence::TryNextRoute` (`Permanent`) never opens
/// a provider's circuit breaker (`backends/crates/sms-provider/src/error.rs`'s own
/// `permanent_never_opens_the_circuit_breaker` guarantee) — a `Permanent`
/// failure is specific to this message (an unapproved sender ID, say), not
/// a provider-wide outage, so nothing else marks that route ineligible.
/// Without remembering it here, a second failover hop could pick the exact
/// same already-failing route right back, and fail identically forever.
///
/// Capped at [`MAX_FAILOVER_HOPS`] (§6.3: "beyond that you're not routing,
/// you're spraying"): once adding the current route to the exclude set
/// would push it past that count, this returns
/// [`FailoverOutcome::Exhausted`] without even querying — the routing pass
/// has nothing to add once the caller isn't willing to act on its answer
/// anyway.
async fn attempt_failover(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    message: &Message,
    reason: &str,
) -> Result<FailoverOutcome, CratestackError> {
    let mut excluded: Vec<String> =
        sms_core::unpack(message.excludedRouteIds.as_deref().unwrap_or(""))
            .into_iter()
            .map(ToOwned::to_owned)
            .collect();
    if let Some(current_route_id) = &message.routeId
        && !excluded.iter().any(|id| id == current_route_id)
    {
        excluded.push(current_route_id.clone());
    }

    if excluded.len() > MAX_FAILOVER_HOPS {
        return Ok(FailoverOutcome::Exhausted);
    }

    let candidate = crate::routing::Candidate {
        operator: message.operator,
        class: message.class,
        app_id: &message.appId,
        msisdn: &message.msisdn,
        message_id: &message.id,
    };
    let exclude_set: sms_routing::ExcludedRouteIds = excluded.iter().cloned().collect();
    let decision = crate::routing::decide(&ctx.db, sys, &candidate, &exclude_set).await?;

    let Some(winner) = decision.winner else {
        return Ok(FailoverOutcome::Exhausted);
    };

    let excluded_route_ids = sms_core::pack(&excluded)
        .expect("Route.id is a Cuid — never empty, never contains the sentinel separator");
    write_failover(
        ctx,
        sys,
        message,
        &winner,
        &excluded_route_ids,
        format!(
            "failover (hop {} of {}): {reason}; rerouted to route {}",
            excluded.len(),
            MAX_FAILOVER_HOPS,
            winner.route_id
        ),
    )
    .await;
    Ok(FailoverOutcome::Rerouted)
}

/// `routed -> queued` with a *new* `providerId`/`routeId` stamped — #63's
/// own reroute write. Deliberately mirrors `claim::apply_routing_decision`'s
/// `accepted` winner branch, not `write_transition`'s backoff scheduling: a
/// failover retry through a *different* provider has no reason to wait, so
/// `leaseUntil` is set to `now` (already expired by the time the very next
/// poll tick reads it), the same "immediately claimable, not in-flight
/// work" pattern that branch already uses.
async fn write_failover(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    message: &Message,
    winner: &sms_routing::Winner,
    excluded_route_ids: &str,
    reason: String,
) {
    let now = chrono::Utc::now();
    if let Err(error) = ctx
        .db
        .message()
        .update(message.id.clone())
        .set(UpdateMessageInput {
            state: Some(MessageState::queued),
            providerId: Some(Some(winner.provider_id.clone())),
            routeId: Some(Some(winner.route_id.clone())),
            excludedRouteIds: Some(Some(excluded_route_ids.to_owned())),
            stateReason: Some(Some(reason)),
            leaseUntil: Some(Some(now)),
            ..Default::default()
        })
        .if_match(message.version)
        .run(sys)
        .await
    {
        log_write_failure(
            &message.id,
            MessageState::queued,
            &map_database_error(error),
        );
    }
}

/// `routed -> submitted` on a successful [`sms_provider::SmsProvider::submit`].
/// `submittedAt` is stamped by the same trigger that enforces the
/// transition table (§2.10) — never set explicitly here.
async fn write_submitted(
    ctx: &WorkerContext,
    sys: &CratestackContext,
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
        log_write_failure(
            &message.id,
            MessageState::submitted,
            &map_database_error(error),
        );
    }
}

/// `routed -> queued` (backoff, `leaseUntil` set to enforce the delay
/// through the same mechanism [`crate::claim`]'s reclaim uses),
/// `routed -> failed`, or `routed -> uncertain`, per
/// [`handle_submit_error`]/[`terminal_outcome`]'s outcome.
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
    sys: &CratestackContext,
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
        log_write_failure(&message.id, next_state, &map_database_error(error));
    }
}

/// §6.3: "five consecutive `Unavailable` opens it for 60s." Same shape as
/// `hooks::record_endpoint_failure` — the decision itself is
/// `breaker::on_failure`, shared with that function; see `breaker.md` for
/// why a second, independently-derived breaker here would be worse than a
/// shared one, and why the constants still differ (a different spec'd
/// subsystem, not an inconsistency). Only ever called for
/// `RoutingConsequence::OpenCircuitAndTryNextRoute` (i.e. `Unavailable`) —
/// never `Permanent`/`Transient`/anything else, matching
/// `backends/crates/sms-provider/src/error.rs`'s own
/// `permanent_never_opens_the_circuit_breaker` test.
///
/// Best-effort, not a CAS-retry loop, matching `hooks::record_endpoint_failure`'s
/// own reasoning verbatim: a lost race under concurrent `dispatch` workers
/// failing against the same provider at once undercounts by a small
/// amount, which only matters for a heuristic that exists to stop hammering
/// a dead provider, not to bill anyone.
async fn record_provider_failure(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    provider: &Provider,
) {
    let now = chrono::Utc::now();
    let decision = breaker::on_failure(&PROVIDER_BREAKER, provider.consecutiveFailures, now);

    let mut set = UpdateProviderInput {
        consecutiveFailures: Some(decision.consecutive_failures),
        ..Default::default()
    };
    if decision.opened_circuit() {
        set.circuitOpenUntil = Some(decision.circuit_open_until);
        warn!(
            provider_id = %provider.id,
            threshold = PROVIDER_BREAKER.failure_threshold,
            "provider circuit breaker opened after consecutive Unavailable failures"
        );
    }

    if let Err(error) = ctx
        .db
        .provider()
        .update(provider.id.clone())
        .set(set)
        .if_match(provider.version)
        .run(sys)
        .await
    {
        warn!(provider_id = %provider.id, %error, "recording provider failure count failed");
    }
}

/// A successful submit resets the breaker — same reasoning as
/// `hooks::reset_endpoint_failures`, including skipping the write entirely
/// when there is nothing to reset, so a healthy provider's every single
/// success doesn't cost a pointless `UPDATE`.
async fn reset_provider_failures(
    ctx: &WorkerContext,
    sys: &CratestackContext,
    provider: &Provider,
) {
    if !breaker::needs_reset(provider.consecutiveFailures, provider.circuitOpenUntil) {
        return;
    }
    if let Err(error) = ctx
        .db
        .provider()
        .update(provider.id.clone())
        .set(UpdateProviderInput {
            consecutiveFailures: Some(0),
            circuitOpenUntil: Some(None),
            ..Default::default()
        })
        .if_match(provider.version)
        .run(sys)
        .await
    {
        warn!(provider_id = %provider.id, %error, "resetting provider failure count failed");
    }
}

/// The only realistic cause of this write failing is a human operator
/// concurrently cancelling the same message — a legitimate race, not a
/// bug, so this logs and lets the tick continue rather than treating it as
/// fatal.
fn log_write_failure(message_id: &str, attempted_state: MessageState, error: &CratestackError) {
    warn!(
        message_id,
        attempted_state = ?attempted_state, %error,
        "writing dispatch's own transition failed — likely a concurrent operator action"
    );
}

#[cfg(test)]
mod tests {
    use super::{UNAVAILABLE_BACKOFF, budget_for, decode_encoding, terminal_outcome};
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
        let (state, _, backoff) = terminal_outcome(&ProviderError::Transient {
            retry_after: Duration::from_secs(5),
            message: "rate limited".to_owned(),
        });
        assert_eq!(state, MessageState::queued);
        assert_eq!(backoff, Some(Duration::from_secs(5)));
    }

    #[test]
    fn unavailable_backs_off_with_a_fixed_delay_and_stays_queued_once_failover_is_exhausted() {
        let (state, _, backoff) = terminal_outcome(&ProviderError::Unavailable {
            message: "connection refused".to_owned(),
        });
        assert_eq!(state, MessageState::queued);
        assert_eq!(backoff, Some(UNAVAILABLE_BACKOFF));
    }

    #[test]
    fn permanent_fails_outright_once_failover_is_exhausted() {
        let (state, reason, backoff) = terminal_outcome(&ProviderError::Permanent {
            code: "SENDER_ID_NOT_APPROVED".to_owned(),
            message: "sender id not approved".to_owned(),
        });
        assert_eq!(state, MessageState::failed);
        assert_eq!(backoff, None);
        assert!(reason.contains("no alternate route"));
    }

    #[test]
    fn rejected_and_unsupported_both_fail_outright_with_no_failover() {
        let (rejected_state, _, rejected_backoff) = terminal_outcome(&ProviderError::Rejected {
            code: "INVALID_DESTINATION".to_owned(),
            message: "bad number".to_owned(),
        });
        assert_eq!(rejected_state, MessageState::failed);
        assert_eq!(rejected_backoff, None);

        let (unsupported_state, _, unsupported_backoff) =
            terminal_outcome(&ProviderError::Unsupported);
        assert_eq!(unsupported_state, MessageState::failed);
        assert_eq!(unsupported_backoff, None);
    }

    /// The whole point of #119, unchanged by #63: an indeterminate outcome
    /// must land in `uncertain`, not `queued` (which `claim.rs`'s
    /// `candidates()` would pick straight back up and resubmit) and not
    /// `failed` (which would discard a message that might still resolve
    /// via a late DLR) — and, per #63's own mapping, `handle_submit_error`
    /// never even calls `attempt_failover` for this variant, so this outcome
    /// is reached directly, not as a "failover exhausted" fallback the way
    /// `Permanent`/`Unavailable` above are.
    #[test]
    fn indeterminate_lands_in_uncertain_with_no_backoff_and_no_retry() {
        let (state, reason, backoff) = terminal_outcome(&ProviderError::Indeterminate {
            message: "read timeout after the request was sent".to_owned(),
        });
        assert_eq!(state, MessageState::uncertain);
        assert_eq!(backoff, None);
        assert!(reason.contains("not retrying"));
    }
}
