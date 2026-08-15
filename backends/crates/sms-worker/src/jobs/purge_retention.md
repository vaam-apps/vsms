`purge_retention` — #67 (M6). §7.5's own table: "Enforce `@@retain`:
null `body`/`msisdn` past 90 days, delete receipts past 90."

# The scope this job does *not* have, and why

#67's own issue text described a "split ledger": purge content and
plaintext MSISDN at 90 days, but keep a second, minimal traffic-metadata
table (timestamp, hashed MSISDN, operator, segments, state) for ten years
to satisfy Law No. 2010/012 art. 25's statutory retention. That was
blocked on decision #5. **The maintainer resolved #5 on 2026-08-11: 90-day
minimisation, no split ledger.** vsms purges at 90 days and does not carry
a parallel ten-year ledger in its own schema; long-horizon retention, if
it is ever required, is an infrastructure concern for whoever operates
the deployment, outside this application. See `docs/architecture.md` §10
(updated in the same PR as this job) and issue #5's own resolution
comment. This job is therefore the *entire* #67 scope, not half of a
two-table design.

# This job's own `.update()` is a real emitting write — it must never
# re-notify a customer about a message they were already told about

`Message` carries `@@emit(created, updated)` (§2.5), and this job's
`purge_messages` writes through the same `.update()` delegate every
other writer of `Message` uses — no exception, R1 gives none. That
means every purge is, structurally, an event: found by the
coordinator's review, not by this file's own first draft, which
reasoned about `msisdn`'s placeholder-vs-nullable tradeoff without
ever checking that `Message.updated` reaches
`backends/crates/sms-api/src/webhooks.rs`'s subscriber, and it does, in every
process that runs this job (`register_subscribers` is called
unconditionally in `backends/apps/sms-worker`'s `main`, regardless of `--roles`
— see that module's own doc for why). Four of this job's five terminal
candidate states map to a catalogued webhook event
(`webhooks.rs::message_event_type`; only `rejected` doesn't), so
without a guard, purging a message would enqueue — and `hooks` would
then sign and POST — a live webhook to the customer's own endpoint,
three months after the fact, carrying whatever the purge just wrote
into `msisdn`. The fix lives at the subscriber, not here: R1 gives no
seam to suppress an emit from the write site, and
`enqueue_message_webhook_attempts` is where every other "should this
state produce an event" decision already lives — see that function's
own doc in `webhooks.rs` for the `purgedAt.is_some()` guard and why
`webhook_attempts_dedupe`'s unique index is a coincidence, not a
substitute for it.

# What gets purged, and why each column

Only `Message` rows in a **terminal** state
(`delivered`/`failed`/`expired`/`rejected`/`cancelled` — §7.4's own
terminal set, the states with no outgoing edge in
`message_state_transitions`) past [`RETENTION`] are touched. A
non-terminal row past 90 days would mean something else is already
broken — §7.4's own backoff/expiry machinery (`expire_stale`, #122)
guarantees every message reaches a terminal state well inside 90 days
(`expiresAt` tops out at 24h for `notification`, plus `uncertain`'s 6h
grace) — so this job never forces a still-live row over that guarantee;
it trusts it.

Per-column reasoning, since "purge" does not mean "delete the row" —
`Message` has no `@@allow` for `delete` (§2.5) and this job doesn't add
one, matching the model's own "the purge is a job [that redacts], not a
deletion" design:

- **`msisdn`** — the plaintext recipient number. Purged: this is the
  decision's own explicit text ("content *and* plaintext MSISDN both").
  The column is `String`, `NOT NULL`, `@length(min: 12, max: 15)` with no
  `@db_enforce` (so nothing enforces the length at the database layer —
  confirmed against `0001_init`, no `CHECK` on this column — but
  `UpdateMessageInput::validate()` still runs the length check in
  application code on every write, `Some`-wrapped values included, since
  update-input validators treat every field as present-or-absent, not
  nullable-or-not). Making the column itself nullable was considered and
  rejected in favour of overwriting it with a fixed, obviously-not-a-number
  placeholder — but the original reasoning here was wrong about *why*,
  caught by the coordinator's review rather than by this file's own first
  draft, and is worth recording precisely because it was wrong.

  The first draft claimed the two production readers of `Message.msisdn`
  — `backends/crates/sms-worker/src/dispatch.rs`'s submit path and
  `backends/crates/sms-api/src/webhooks.rs`'s `Message.created`/`updated`
  subscriber — could structurally never see a purged row, so
  `Option<String>` would only add risk to two already-correct hot paths
  for no benefit. That is true of `dispatch.rs` (its candidate set is
  `accepted`/`queued`/`routed`/`undelivered`, never a terminal state this
  job touches) and **was false of `webhooks.rs`**: this job's own
  `.update()` call is a real delegate write against a model with
  `@@emit(created, updated)`, so it fires the exact same subscriber every
  other `Message` update does, and four of this job's five terminal
  candidate states map to a catalogued event
  (`backends/crates/sms-api/src/webhooks.rs`'s own `message_event_type`).
  Un-caught, that meant every purge attempted to enqueue — and `hooks`
  would then sign and POST — a live webhook to the customer's endpoint
  reporting on a message three months stale, carrying whatever
  `message.msisdn` held at that moment. Fixed at the source in
  `webhooks.rs`: `enqueue_message_webhook_attempts` now returns `Ok(())`
  immediately when `message.purgedAt.is_some()`, before it ever reads
  `msisdn` — see that function's own doc for the full reasoning and why
  `webhook_attempts_dedupe`'s unique index is not a substitute for this
  guard.

  With that fixed, the placeholder decision's actual justification
  holds: nothing in this codebase reads a purged message's `msisdn` for
  any production purpose any more — `dispatch.rs` structurally, and
  `webhooks.rs` because of the guard above — so `Option<String>` would
  force `.expect()`-shaped defensive handling into two heavily-tested,
  already-correct hot paths for a branch that can never be hit, in
  exchange for no privacy benefit `PURGED_MSISDN_PLACEHOLDER` doesn't
  already give.
- **`body`** — the message content. Purged: the decision's own explicit
  text ("content"). Nullable already (`String?`), so a plain `None`.
- **`clientRef`** / **`idempotencyKey`** — caller-supplied correlation
  strings from the *App's own* system (an order id, a ticket id, in some
  integrations literally a customer id or email — vsms has no control
  over what an App puts here, per `sendMessage`'s own doc: "`clientRef`
  is the only caller-supplied correlation string `SendMessageInput`
  carries"). Genuinely customer-identifying in the common case, which is
  exactly why the issue flagged both as arguable. Purged, both, for the
  same reason — `idempotencyKey` is a verbatim copy of `clientRef` at
  creation time (`procedures.rs`: `let idempotency_key =
  args.clientRef.clone();`), so leaving one plaintext while nulling the
  other would purge nothing.
- **`stateReason`** — human-readable text written by
  `backends/crates/sms-worker/src/dispatch.rs`'s `classify()` from
  `ProviderError::{Permanent,Rejected,Indeterminate}`'s own `message`
  field, i.e. free text straight from Orange's API response, not a
  closed set of vsms-authored strings. Whether that text can ever embed
  subscriber-identifying detail is unverified either way, and the
  near-term diagnostic value of a specific provider error string is gone
  long before 90 days. Purged for minimisation, not because a concrete
  leak was found.
- **`purgedAt`** — stamped `Some(now)` the moment a row is purged. See
  the field's own schema.cstack doc comment for why an explicit marker
  beats inferring "purged" from "`body` is null": nothing else in this
  codebase ever nulls `body` today, but relying on that silently as an
  implicit contract is exactly the shape of gap this codebase keeps
  finding elsewhere. `purgedAt` is also this job's own idempotency guard
  (`purgedAt IS NULL` is part of the candidate filter) and what lets a
  live test assert "purged" as a fact rather than inferring it from a
  side effect.

**Left alone, deliberately, each for a different reason:**

- **`msisdnHash`** — required by the decision's own text to survive:
  "`msisdnHash` remains after a purge and is what any post-purge
  correlation (opt-out matching, dedupe) runs on." See `sms_api::pepper`'s
  module doc for the sharper consequence this decision creates: rotating
  `SMS_HASH_PEPPER` does not rehash stored rows, and a row whose plaintext
  `msisdn` has already been purged **can never be rehashed** — a pepper
  rotation permanently breaks matching against every already-purged row.
  This job is what makes rows permanently unrehashable, so it is the
  right place to restate that caveat, not just `pepper.rs`.
- **`bodyHash`** — also `NOT NULL`, so purging it would need the same
  placeholder workaround as `msisdn` for materially less privacy benefit:
  `#134`'s own audit (`rg bodyHash` across this crate and the admin
  console's generated TS client) already established it is write-only,
  never read back by anything, peppered under the same scheme as
  `msisdnHash`. Left in place as a known, considered gap rather than
  solved here.
- **`senderIdValue`** — identifies which brand/App sent the message, not
  who received it. §2.5's own linkage argument ("`senderIdValue` and
  `appId` sit beside a plaintext `msisdn`, so 'this number has an account
  with this brand' is present") needed the plaintext `msisdn` to be
  meaningful; once `msisdn` is purged, `senderIdValue` next to a peppered
  `msisdnHash` doesn't reconstitute that linkage without the pepper.
- Everything else (`operator`, `class`, `priority`, `bodyLength`,
  `encoding`, `segments`, `state`, `routeId`, `providerId`,
  `providerMessageRef(Alt)`, `attempts`, `maxAttempts`, `costXaf`,
  timestamps) is operational or billing metadata, not recipient PII.

`DeliveryReceipt` gets the doc's other verb, "delete", not "null": the
model carries no correlation-key column worth preserving the way
`Message.msisdnHash` is, and `rawPayload` is an opaque provider blob this
codebase has no principled way to partially redact — §2.5's own prose
already called receipts "append-only"; this job is the first thing that
ever removes one. Purge eligibility is the row's own `receivedAt`
(`@@retain(days: 90)` on `DeliveryReceipt` is independent of `Message`'s),
not its parent message's age.

# `DeliveryReceipt.delete` is a new capability, not a new exception

`DeliveryReceipt` had no `@@allow("delete", ...)` clause at all before
this PR — not a broken clause, an *absent* one, which this codebase's own
deny-by-default framework makes indistinguishable from "deliberately
unsupported" until something actually needs it. This job is that
something. `schema.cstack` only; no DDL consequence (an `@@allow` change
never touches emitted DDL — confirmed the same way every prior instance
in this file was: a byte-identical `cratestack migrate diff` before and
after).
