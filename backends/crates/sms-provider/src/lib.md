The abstraction every SMS provider adapter — HTTP or SMPP — fits behind.
§6.1 of the design doc.

Pure, like `sms-encoding` and `sms-msisdn`: no `cratestack` dependency,
no schema types. [`SmsProvider`] is what `sms-worker`'s `dispatch` role
(#33) calls through; concrete adapters (`sms-provider-orange-cm`,
`sms-provider-mtn`, `sms-provider-aws-sms`, and later a config-driven aggregator and SMPP) are
separate crates that implement it. Nothing here decides *which* provider gets a message — that's
routing (§6.3), a `sms-worker` concern, not this crate's.

Two things carry the whole design, and both are types, not prose a
caller has to remember:

- [`Capabilities`] — what a provider can do, so routing asks
  `capabilities.ucs2` instead of special-casing a provider's identity.
- [`ProviderError`] and [`error::RoutingConsequence`] — what went wrong,
  mapped to exactly one routing decision by a compiler-checked match
  rather than a comment. See the module doc on [`ProviderError`] for why
  this is the part of the whole provider layer most worth getting right.

A third piece is a hook rather than a type: [`SmsProvider::authenticate_callback`]
and its result, [`CallbackDisposition`] (`Ingest`, `Acknowledged { detail }`,
`Refused { reason }`). The gateway's `POST /dlr/{providerKey}` route is public
and unauthenticated, so something has to decide whether a callback is genuine
before [`SmsProvider::parse_dlr`] and the database see it. That decision is a
separate async step because `parse_dlr` is synchronous and pure, and checking
Amazon SNS's signature needs a certificate fetch (and, for a subscription
handshake, an outbound request). It defaults to `Ingest`, which is right for
Orange and MTN: neither documents a signature scheme, so there is nothing to
check. An adapter whose provider does sign its callbacks must override it,
because the default trusts every caller. Only `aws_sms` does today.

The three outcomes map to HTTP in the gateway: `Refused` is `403` and the body
is never parsed; `Acknowledged` is `200` with nothing parsed (a handshake the
adapter already dealt with); `Err` means the check could not run and is `503`,
so the provider redelivers instead of a genuine receipt being discarded. An
`Err` here must not be used to say "forged".

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
