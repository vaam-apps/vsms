Parsing MADAPI's `DeliveryNotificationRequest` — the shape the vendored
Swagger documents for what "will be sent to a third party endpoint to
report the status of a message delivery"
(`mtn-sms-v3-swagger.yaml`, `definitions.DeliveryNotificationRequest`).
See `lib.md` for the provenance of that file and what is and isn't
verified about it.

**Every property on `DeliveryNotificationRequest` is marked optional in
the schema** — there is no `required` list on this definition at all,
unlike `outboundSMSMessageRequest`/`resourceReference`. That is either a
documentation looseness (a real DLR presumably always carries a status)
or a genuine "any field may be absent" contract; this module assumes the
former is the common case and the latter is possible, so it degrades
rather than rejects wherever it safely can:

- **`clientCorrelatorId` is the one field this module still treats as
  effectively required**, because it is the caller-supplied
  `clientCorrelatorId` echoed back from `submit`'s own request body —
  without it there is nothing to correlate a `DeliveryUpdate` against a
  `Message` at all, the same reasoning
  `sms-provider-orange-cm::dlr::parse` uses for its own required
  `callbackData`. A notification carrying none (or an empty string) is
  rejected outright.
- **`deliveryStatus` missing is not rejected** — it maps to
  [`DeliveryOutcome::Unknown`] with an empty `raw_status`, the same
  honest "we don't know" this module already gives an unrecognised
  status string. A malformed body is a caller (MTN) problem worth
  surfacing loudly; a notification that's merely thin on detail is not.
- **`completedDate` is parsed as RFC3339 and a parse failure does not
  fail the notification.** The Swagger types it `format: datetime` with
  no further specification of which datetime format — RFC3339 is the
  one this codebase already assumes everywhere else a wire timestamp
  needs interpreting with no stronger signal, but it is a guess against
  an undocumented format, not a confirmed one. A value that fails to
  parse is logged (`tracing::warn!`) and `occurred_at` is left `None` —
  `DeliveryUpdate::occurred_at` is itself documented as `Option`, and
  `DeliveryReceipt.receivedAt` (stamped by the database, not this field)
  already carries a reliable timestamp regardless, so losing this one
  optional, provider-reported field is a diagnostic downgrade, not a
  correctness one.

**`error` and `details` both map onto [`DeliveryUpdate::error_code`],
`error` first.** The Swagger gives no indication of which is the more
stable/machine-readable of the two — `error` reads as the more
code-shaped name, `details` as free text — so `error` is preferred when
both are present, and `details` is used only as a fallback when `error`
is absent, so a failure notification with only free text still carries
*something* in `error_code` rather than nothing. There is no second slot
on [`DeliveryUpdate`] to carry both, so whichever is not chosen is
simply lost; this is a real, accepted information loss, not an oversight.

**`delivering_network` is always `None`.** `DeliveryNotificationRequest`
has no field naming the network that actually delivered the message —
unlike the wider aggregator vocabulary Orange's own `dlr.rs` was once
compared against, MADAPI's schema carries `senderAddress`/
`receiverAddress` but nothing analogous. [`DeliveryUpdate::delivering_network`]'s
own doc already says `None` is the correct, honest answer when a
provider's DLR shape doesn't carry this — this is exactly that case, not
a placeholder for a future fix.

**The eight-value `status` enum is matched exhaustively against the
known set, with a trailing wildcard for anything else** — see `lib.rs`'s
own `outcome_of` for the mapping and the reasoning behind the two least
obvious entries (`ACCEPTD`/`ENROUTE` → [`DeliveryOutcome::InFlight`],
`DELETED` → [`DeliveryOutcome::Rejected`]). Nothing here should ever
guess an unrecognised status into a terminal outcome — see
[`DeliveryOutcome::Unknown`]'s own doc for why that would be worse than
useless to an operator debugging a real failure.
