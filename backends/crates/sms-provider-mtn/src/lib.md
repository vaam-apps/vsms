[`SmsProvider`] for MTN Cameroon capacity bought through a licensed
aggregator, not a direct MTN interconnect. #61 — see the epic (#60) and
`docs/architecture.md` §6.2/§6.4: MTN's own developer portal publishes no
pricing, no sender-ID policy, and no DLR spec for Cameroon, and the
commercial path there routes through MTN's local enterprise team on a
timeline this repo cannot build against. Buying capacity through a
licensed aggregator is the recommended posture in §6.4 for exactly this
reason, and it is also the path that avoids the ART-title question
(decision #4) entirely — this crate never opens an SMPP bind or an
interconnect of its own.

# This is a provisional shape, not a verified integration

**No real aggregator contract, credentials, or API document exists in
this repo, and none of this has been run against a live endpoint.**
Matching `sms-provider-orange-cm`'s own precedent of naming exactly how
confident each part is (see that crate's module doc on the OAuth/submit
shape vs. the DLR shape), here is the honesty ledger for this crate:

- **The transport-error classification** (connect-vs-read,
  [`ProviderError::Indeterminate`] on a post-connect timeout or an
  unparseable/incomplete `2xx`) is provider-agnostic reasoning, not an
  aggregator-specific claim. It is exactly as trustworthy here as it is
  in `sms-provider-orange-cm`, because it follows from what `reqwest`
  itself guarantees about `is_connect`/`is_timeout`/`is_body`, not from
  any aggregator's documentation — which is exactly what let it move,
  unchanged, into `sms-provider-http` once this crate proved a second
  adapter needed the identical reasoning. Same for the provider-agnostic
  half of the HTTP-status mapping (`429` → `Transient`, `5xx` →
  `Unavailable`, everything else → `Rejected`) — see
  `classify_submit_error` in this crate's own source for what stays
  local (`401`/`403`) and why.
- **The request/response JSON shape below (`POST /v1/messages`, Bearer
  API-key auth, a `messageId` in a `201` response, a `POST` DLR callback
  carrying that same `messageId`) is an invented, best-guess shape**,
  chosen to match the common pattern across the aggregators
  `docs/architecture.md` §6.2 already names as candidates for this route
  (Nexah, Africa's Talking, Infobip, Twilio): a REST `POST` that returns
  `201 Created` for a newly created message resource (Africa's Talking's
  own SMS API does exactly this), a JSON body carrying the created
  message's id, and a webhook DLR that echoes that same id back. It is
  **not** transcribed from any one vendor's real Swagger/API reference
  the way Orange's submit shape was transcribed from §6.2. Treat every
  field name below as a placeholder until a real contract exists, and
  replace this module's request/response structs — not the
  [`SmsProvider`] impl or the error classification around them — the
  moment one does.
- **Auth is a static Bearer API key, not `OAuth2 client_credentials`.**
  Chosen over mirroring Orange's OAuth dance because a static
  aggregator-issued key is at least as common a pattern in this space
  (Africa's Talking, Infobip, and most SMS aggregators issue a key or
  App-ID/App-secret pair rather than running a token endpoint), and it
  avoids inventing a second unverified token-endpoint shape on top of an
  already-invented submit shape. If a real MTN aggregator contract turns
  out to use `client_credentials` instead, `sms-provider-orange-cm`'s
  `token.rs` is the pattern to copy — nothing else in this crate would
  need to change.

# `Capabilities` is genuinely different here, not just re-declared

This is the actual point of #61's second paragraph, and it shows up as a
structural difference from `sms-provider-orange-cm`, not just different
field values. Orange's `capabilities()` (`sms-provider-orange-cm/src/lib.rs`)
is a bare function with no inputs — every field is a fact about Orange's
self-service product that's true for every deployment of this codebase.
MTN-via-aggregator has no such fixed facts: `tps_ceiling` and
`cost_per_segment_xaf` are negotiated per aggregator contract, not
published anywhere, and whether an alphanumeric sender ID is usable at
all depends on that specific contract's own sender-ID registration
status with MTN (§3.3 of `docs/architecture.md`: "MTN requires
pre-registration through your aggregator" — a per-relationship fact, not
a platform-wide one the way Orange's support-form whitelist is). So
[`MtnAggregatorConfig`] carries `tps_ceiling`, `cost_per_segment_xaf`,
and `supports_alphanumeric_sender` as caller-supplied fields, and
[`MtnAggregatorProvider::capabilities`] reads them back rather than
returning a compiled-in constant. A routing layer that special-cased
`if key == "orange_cm" { 5.0 } else { some_other_constant }` would be
exactly the anti-pattern `sms-provider`'s own module doc warns against;
reading `capabilities().tps_ceiling` off whichever provider is
configured is the only version that survives a second aggregator
relationship with different contract terms.

One `Capabilities` field this crate's DLR shape makes genuinely simpler
than Orange's, worth recording precisely because it's the opposite
direction of complexity: Orange's DLR (`sms-provider-orange-cm/src/dlr.rs`)
cannot correlate on the same reference it returns from `submit` — it
needs `receiptRequest.callbackData` and `SubmitAck::provider_ref_alt` to
work around that (#95). This crate's assumed DLR shape echoes the same
`messageId` `submit` already returns, so `provider_ref_alt` is always
`None` here. That is a property of the *assumed* shape, not a proven
simplification — if a real MTN aggregator's DLR turns out to reference
the submission differently, this is exactly the kind of correlation gap
#95 already shows this codebase can silently ship with, so revisit
`dlr::parse` and `submit`'s `provider_ref_alt` together the moment a real
payload is available.
