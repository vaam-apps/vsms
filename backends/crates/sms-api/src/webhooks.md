#38 — subscribers that turn `@@emit`'d model events into `WebhookAttempt`
rows. See §8 of the design doc for the full design; this module doc
covers the two things that shaped its code and aren't obvious from
reading it cold.

# The hard constraint every function here is written against

`@@emit` delivery (`cratestack_event_outbox` → `CratestackEventBus::emit`,
§8.2) is **synchronous, blocks the mutation that triggered it, and is
not panic-isolated**. A subscriber that blocks or panics breaks
`sendMessage`, `dlr::ingest`, or whatever else touched an emitting
model. So every subscriber here does exactly one thing — read the
event, look up matching `WebhookEndpoint` rows, insert `WebhookAttempt`
rows — and [`register_subscribers`] wraps each one in `tokio::spawn` so
a bug becomes a logged `Err` (a `JoinError`) rather than an unwind
through the mutation's own call stack. No HTTP call, no retry, no
branching beyond "does this state map to a catalogued event type" and
"which endpoints subscribe to it" — all real delivery is the `hooks`
role's job (M3 #40), not this module's.

# Resolving #38 vs #39: if subscribers already insert attempts
synchronously, what does `drain` (#39) drain?

`db.events().on_message_created(...)`/`on_message_updated(...)` each
register against a `Cratestack`/`SqlxRuntime` instance's own **in-process**
`CratestackEventBus` (`cratestack_sqlx::descriptor::SqlxRuntime::subscribe`,
read directly in the vendored source, not assumed) — registration
never crosses a process boundary, and *every* `@@emit`-annotated
mutation triggers an automatic drain of its own process's runtime
immediately after commit (`cratestack-sqlx`'s `create.rs`/`update.rs`:
`let _ = self.runtime.drain_event_outbox().await;`, unconditional, no
`db.events().drain()` call required to trigger it).

That has a sharp edge, and it is the actual answer to the question
above: **`CratestackEventBus::emit` returns `Ok(())` for a topic with zero
registered handlers** (`cratestack-core/src/events/bus.rs`: an empty
handler `Vec`, an empty `for` loop, `Ok(())`) — not an error, not a
skip flagged anywhere. So a process that writes to an emitting model
(`Message`, in this milestone's scope) *without* having called
[`register_subscribers`] on its own `Cratestack` instance first does
not "leave the row for `drain` to pick up later." Its own automatic
post-commit drain call marks the row `delivered_at = NOW()`
immediately, having done nothing, and `drain_event_outbox`'s own
`SELECT ... WHERE delivered_at IS NULL` never sees it again. The event
is not stalled; it is **lost, silently, the moment the write
commits** — a worse failure mode than the one #39 names.

That makes [`register_subscribers`] mandatory plumbing in **every**
process whose own `Cratestack` instance ever writes to an emitting
model, not optional wiring for wherever the `drain` role happens to be
scheduled: `backends/apps/sms-gateway` (`sendMessage`, `dlr::ingest`, both write
`Message`) and `backends/apps/sms-worker` (`dispatch` writes `Message`;
`jobs::expire_stale` writes it too). `backends/apps/sms-worker` registers once in
`main`, before any role task is spawned, against the one `Cratestack`
every role's `WorkerContext` clones — `Cratestack`/`SqlxRuntime`/
`CratestackEventBus` all derive `Clone` over `Arc`-backed state, so a clone
shares the same live handler registry, not a copy of it. One
registration call covers every role that process runs, including ones
that never touch an emitting model themselves (`hooks`, `jobs`) — those
registrations just sit unused, which costs nothing.

Given all of that, what `crate::drain`'s `drain` role (#39, in
`backends/crates/sms-worker`) actually adds on top of every writer's own
automatic post-commit drain is exactly the one thing no writer path
gives you: a handler that failed on its first attempt (a transient
error creating the `WebhookAttempt` row, say) is left
`delivered_at IS NULL` with `attempts`/`last_error` recorded by
`drain_event_outbox` itself, and **nothing retries it until the next
drain** — which, without a write-independent trigger, only happens
whenever the next mutation on *any* emitting model happens to occur.
`drain`'s periodic call, unconditional on any mutation happening at
all, is that trigger — the literal fix for "the framework runs no
background drain worker" (§8.2).

# `WebhookEndpoint`'s missing `hasRole('system')` — the eighth instance

[`enqueue_message_webhook_attempts`] reads `WebhookEndpoint` under a
`system` context to find which endpoints subscribe to a given event
type. Before this change `WebhookEndpoint`'s `@@allow("read", ...)`
clause was `auth().kind == "user"` only — the same shape `AGENTS.md`'s
"Invariants that fail the build rather than production" section has
recorded seven times before (`App`, `AppClient`,
`SenderIdRegistration`, `OperatorPrefixRule`, `Provider`, `Job`,
`DeliveryReceipt`): a missing `hasRole('system')` clause doesn't error,
it silently filters a system context's read down to an empty array.
`schema.cstack` now adds it (policy-only — no DDL consequence, so
`0001_init` was not regenerated, per `AGENTS.md`'s own standing rule);
`backends/crates/sms-api/tests/system_context_golden_list_live_postgres.rs`
moves `WebhookEndpoint` into `SYSTEM_READABLE_MODELS` to match. Found
here, by this PR's own live suite, before merge — not live in
production, which is the entire point of that golden test existing.

# The stored payload is `data` only, not a full envelope

§8.4's example webhook body has an outer envelope (`id`, `type`,
`occurredAt`) wrapping a `data` object. This module stores only the
`data` object in `WebhookAttempt.payload` — the outer `id` in that
example is not `Message.id` (the example's own `data.messageId` is a
*different* id from the top-level `id`), and the only value that
naturally supplies a distinct, stable, per-attempt id is
`WebhookAttempt.id` itself, which does not exist yet at the moment
this subscriber runs (it's the row this subscriber is in the middle of
creating). Building the final signed envelope — `id` from the
`WebhookAttempt` row, `type` from its `eventType` column, `occurredAt`
from its `createdAt` — is naturally the `hooks` role's job (#40) at
the moment it actually POSTs, alongside signing (#41), not this
module's.
