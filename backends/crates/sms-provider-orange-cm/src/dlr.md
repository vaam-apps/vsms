Parsing Orange's delivery notification callback.

**Not verified against a live Orange sandbox** — this repo has no Orange
Developer credentials, and §6.2 documents the submit path in detail but
not the DLR callback's JSON shape. What's implemented here follows the
`deliveryInfoNotification` shape common to the GSMA `OneAPI` SMS family
Orange's own outbound API belongs to (the same lineage as the
`outboundSMSMessageRequest` shape §6.2 *does* specify for submission).
Treat this module as the best available design until it can be checked
against a real callback payload, and add a fixture from Orange's sandbox
the moment one exists — see `parses_a_delivered_notification` below for
where it would slot in.

# Correlation: fixed per #95, grounded in public `OneAPI` docs, still
# sandbox-unverified

[`DeliveryUpdate::provider_ref`] used to be set from each entry's
`address` field — the destination MSISDN, not the `resource_id` UUID
`submit()` (`lib.rs`) stores as `Message.providerMessageRef`. A phone
number can never equal a UUID, so correlation could never have worked
(#95, caught in review of #94 by two independent bots).

The fix: `submit()` now sends `receiptRequest.callbackData` set to
`SubmitRequest::reference` (`Message.id`) on every outbound request —
confirmed against the public `OneAPI` SMS Messaging REST binding
(Oracle Communications' `OneAPI` reference docs, which describe the same
`outboundSMSMessageRequest`/`deliveryInfoNotification` family §6.2 is
already modelled on) that `callbackData` is "passed back in the
notification, allowing you to identify the message." The notification
echoes it back as a **top-level** field of `deliveryInfoNotification`,
sibling to the `deliveryInfo` array — not per-entry — which is what
[`parse`] now reads as `provider_ref` instead of `address`.

This is still unverified against Orange Cameroon's own live
implementation specifically: `notifyURL`/`callbackData` is documented
generic `OneAPI` behaviour, not confirmed Orange-Cameroon behaviour (the
module doc above's own long-standing caveat). The first real DLR this
adapter ever receives from a live Orange sandbox is also the first live
verification of this fix — if `callbackData` doesn't come back exactly
as sent, capture the raw payload and revisit.
