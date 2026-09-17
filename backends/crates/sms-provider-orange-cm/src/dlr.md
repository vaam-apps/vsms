Parsing Orange's delivery notification callback.

# What's documented versus what's still unverified

Grounded in Orange's own developer docs
(<https://developer.orange.com/apis/sms/getting-started>, §4 "About SMS
Delivery Receipt"), not the wider GSMA `OneAPI` family this module used
to reason from by inference. Documented, and implemented here to match
exactly:

- **The DR callback body shape.** `deliveryInfoNotification` carries
  `callbackData` and `deliveryInfo` as siblings; `deliveryInfo` is a
  single JSON **object**, not an array — the assumption this module
  carried before (inherited from the broader `OneAPI` family, never
  Orange's own docs) was wrong, and a real single-object notification
  would have failed to parse at all, rejecting every genuine DLR as
  `MALFORMED_DLR` (HTTP 400). See [`DeliveryInfoField`] for why both
  shapes are still accepted leniently rather than hard-failing on the
  array form.
- **`callbackData` is Orange's own `{{resource_id}}`, not a
  caller-supplied token.** Orange's own words: "the unique ID of your
  previously well sent SMS that you will need to use to correlate the
  corresponding SMS." It's the same id `submit()` (`lib.rs`) already
  recovers from `resourceURL`'s trailing path segment at submit time and
  stores as `Message.providerMessageRef` — one id, reported twice, once
  at submit, once at DR. There is no caller-supplied correlation field
  anywhere in the documented request or response; the maintainer's own
  decision (see `lib.rs`'s module doc) is to stop pretending one exists
  via `receiptRequest`.
- **The `deliveryStatus` vocabulary** — exactly the five values §4's own
  table lists (`DeliveredToTerminal`, `DeliveryUncertain`,
  `DeliveryImpossible`, `MessageWaiting`, `DeliveredToNetwork`), with
  Orange's own caveat that `DeliveredToTerminal` can be relied on but
  `DeliveryImpossible` cannot (see [`outcome_of`]'s own doc for how that
  caveat shapes the mapping).

**Still genuinely unverified: nothing has been received from a real
Orange sandbox.** A documented shape is not the same thing as an
observed one — this repo has no Orange Developer credentials, and every
claim above is a careful reading of the public docs, not a captured
payload. The first real DLR this adapter ever receives from a live
Orange account is still the first live verification of this module,
exactly as it was before this file was rewritten — only the starting
point moved, from "inferred from a different API family" to "read
directly off Orange's own documentation."

# Correlation, in one line

`callbackData` (Orange's own `resource_id`) is matched against
`Message.providerMessageRef`, the value `submit()` stored there from the
`201` response's `resourceURL` at submit time. There is no second,
alternate correlation path any more (see `lib.rs`'s own doc on
`SubmitAck::provider_ref_alt` now always being `None` for this
provider) — a submission whose `resource_id` was never learned (a
timed-out submit, `ProviderError::Indeterminate`) has nothing for a
later DLR to match against and is resolved by `expire_stale` instead.
That's a deliberate, accepted tradeoff, not a gap in this module.
