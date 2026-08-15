I/O glue between the schema and `sms_routing`'s pure engine — #62,
§6.3 of the design doc.

Replaces `claim.rs`'s old `cheapest_active_provider` placeholder as the
thing `Claimable for Message::take_lease`'s `accepted` branch calls:
real `Route`-rule matching (priority, weight, operator/class/app/
prefix predicates) instead of "whichever active provider is cheapest".
See [`decide`]'s own doc for the actual query shape and
[`sms_routing::select_route`]'s doc (in the pure crate this module
wraps) for the algorithm itself — nothing here decides anything; this
module only fetches rows, converts the schema's own enums onto
`sms_routing`'s mirrored ones (the same pattern `dispatch.rs`'s
`decode_encoding` already uses for `Encoding`), and draws the one
random `f64` production needs.

# No routes configured at all

A deployment with zero `Route` rows refuses to dispatch — every
`accepted` message goes to `rejected` with a `stateReason` explaining
why, the same "refuse loudly" posture `cheapest_active_provider` never
had to consider (it always had exactly one thing to pick from: any
active `Provider`). This is a real behaviour change from that
placeholder, weighed deliberately per #62's own scope note rather than
defaulted into: a silent single-provider fallback would mean "which
provider a message goes to" quietly stops being explainable the moment
any `Route` row exists but doesn't match, exactly the failure mode this
ticket exists to close. `backends/crates/sms-api/examples/send_test_message.rs`
(the fixture `just demo` seeds from) now seeds a catch-all `Route`
alongside its `Provider`, so this cutover doesn't leave the demo
silently unable to send.
