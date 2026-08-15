The abstraction every SMS provider adapter — HTTP or SMPP — fits behind.
§6.1 of the design doc.

Pure, like `sms-encoding` and `sms-msisdn`: no `cratestack` dependency,
no schema types. [`SmsProvider`] is what `sms-worker`'s `dispatch` role
(#33) will call through; concrete adapters (`sms-provider-orange-cm`,
and later MTN, an aggregator, SMPP) are separate crates that implement
it. Nothing here decides *which* provider gets a message — that's
routing (§6.3), a `sms-worker` concern, not this crate's.

Two things carry the whole design, and both are types, not prose a
caller has to remember:

- [`Capabilities`] — what a provider can do, so routing asks
  `capabilities.ucs2` instead of special-casing a provider's identity.
- [`ProviderError`] and [`error::RoutingConsequence`] — what went wrong,
  mapped to exactly one routing decision by a compiler-checked match
  rather than a comment. See the module doc on [`ProviderError`] for why
  this is the part of the whole provider layer most worth getting right.
