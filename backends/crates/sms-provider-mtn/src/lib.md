[`SmsProvider`] for MTN via MTN's own **direct API, MADAPI** ("MTN Africa
Developer API Platform"), not the licensed-aggregator posture this crate
originally shipped under for #61. `docs/architecture.md` §6.4 recommended
an aggregator specifically because "MTN's own developer portal publishes
no pricing, no sender-ID policy, and no DLR spec for Cameroon" at the
time — that gap is now closed by a real, vendored contract, and the
maintainer's explicit decision is to build against it directly rather
than keep the aggregator framing. This is a hard cutover: no aggregator
type, config field, or doc claim survives from the previous shape.

# The contract is now real, documented, and vendored — with one honest gap

**`mtn-sms-v3-swagger.yaml`, committed alongside this crate's source, is
the authoritative source for everything this crate builds against.**
Downloaded from MTN's own developer portal
(<https://developers.mtn.com/products/sms-v3-api>) on 2026-09-17. Read
it directly rather than trusting any summary here or elsewhere — Swagger
2.0, `host: api.mtn.com`, `basePath: /v3/sms/`.

**Provenance caveat, stated plainly rather than buried in a commit
message:** `developers.mtn.com`'s TLS certificate was expired at
download time (Let's Encrypt, `notAfter Sep 16 06:02:41 2026 GMT` — one
day before this crate was written), so certificate verification was
bypassed for that one download, at the maintainer's explicit direction.
The certificate chain was otherwise intact (`CN=developers.mtn.com`,
issued by Let's Encrypt) — this was an expired-but-otherwise-legitimate
certificate, not a substituted one, and nothing about the download
mechanism itself was compromised. SHA-256 of the vendored file:
`5a74e8531afd7d087829f668a5c493df157e620dc0480c9213ef5bcee083fb94`.
Anyone re-verifying this crate's shape against the real API should
re-download over a valid certificate and diff against the vendored
copy before trusting either.

**What "documented" does and does not mean here — the one thing this
section exists to be honest about.** Every request/response shape, every
status code, the `OAuth2` flow, and the DLR callback shape below are
transcribed from the vendored Swagger, not invented — a real, material
improvement over this crate's original placeholder shape (`POST
/v1/messages`, a static Bearer key, an invented `messageId`/`status`
envelope — none of that exists any more). **But a documented shape is
not an observed one.** Nothing in this crate has ever been run against a
real MTN account, a real client id/secret pair, or a real webhook
delivery — the same caution `sms-provider-orange-cm/src/dlr.md` draws
for Orange's own DLR shape, which stayed genuinely unverified for a long
time even after its submit shape was confirmed live. Treat every
classification decision this crate makes for an undocumented case (a
`200` response whose `statusCode` isn't `'0000'`, a `401`/`407` at
submit time, which of `error`/`details` a DLR's failure reason lands in)
as a considered guess, not a confirmed fact, until this crate is proven
against a real endpoint.

# Auth: `OAuth2 client_credentials`, matching Orange's own shape

MADAPI's `securityDefinitions.OAuth2` names `flow: application` (RFC
6749's `client_credentials` grant) with a fixed `tokenUrl`. This
replaces the previous crate's invented static-Bearer-key auth entirely —
[`MtnConfig`] now carries `client_id`/`client_secret`, not `api_key`.
[`token`]'s own module doc covers the two respects in which MADAPI's
token endpoint genuinely differs from Orange's (an absolute URL on a
different path than the SMS API's own `basePath`, and
`grant_type=client_credentials` already on the query string rather than
the form body) — read it before touching [`MtnProvider::access_token`]
or [`token::fetch`].

# `serviceCode` is the fixed, required identity; `senderAddress` is the optional, per-message override

MADAPI's `outboundSMSMessageRequest.serviceCode` is unconditionally
required ("the short code that is provided by the api consumer and is
approved by the opco... This field is mandatory") — the closest MADAPI
analogue to `sms-provider-orange-cm::OrangeCmConfig::sender_number`, and
[`MtnConfig::service_code`] plays that role: a fixed, contract-level
identity, always sent. `senderAddress` is genuinely optional and, per
the Swagger, "takes precedence over the serviceCode" when present.

**Where `senderAddress` comes from is a real design decision this
crate's rewrite had to make, and it is a deliberate departure from the
crate's own previous behaviour.** The original placeholder always sent
one fixed, config-level `sender_id` for every message, ignoring
`SubmitRequest::sender_id` entirely. That was wrong the same way it
would be wrong for Orange: §3.3's sender-ID approval is a per-`App`
relationship, and `Message.senderIdValue` — what `sendMessage` already
resolved as the specific, approved sender for *this* message — is
exactly what `SubmitRequest::sender_id` carries (the identical field
Orange's own `sender_name` reads from `req.sender_id`, not
`OrangeCmConfig`). [`MtnProvider::sender_address`] now reads
`req.sender_id` first, falling back to [`MtnConfig::sender_id`] (an
operator-configured default) only when the per-message value is empty —
which, given `senderIdValue String @length(min: 3, max: 11)` in the
schema, should never happen in practice; the fallback exists for
defensive correctness, not because this crate has observed it being
needed.

**`--mtn-sender-id`/[`MtnConfig::sender_id`] is deliberately *not* part
of the all-or-none required-credential group any more.** The Swagger
marks `senderAddress` optional, and unlike `serviceCode` there is a
fully valid MADAPI submission with no `senderAddress` at all (MTN then
uses the `serviceCode` identity for the message) — requiring an operator
to set a value that MADAPI itself treats as optional, purely because
this crate's *previous* shape treated it as load-bearing, would be
carrying the old design's requirement forward for no reason the new
contract gives.

# What a `200` on `/messages/sms/outbound` actually means

Unlike Orange's `201 Created`, MADAPI's own documented success response
for a submit is `200` ("Outbound SMS created" — the Swagger's own
description text, despite the `200` status) carrying a `resourceReference`
envelope: `statusCode` (a **4-character MADAPI canonical error code**,
`'0000'` on success), `statusMessage`, `transactionId`, and a `data`
object. **A `200` alone is not success** — see
[`classify_application_error`]'s own doc: the HTTP layer answering `200`
only means the request reached MADAPI and MADAPI made a decision about
it; `statusCode != "0000"` is MADAPI's own, explicit way of reporting an
application-level failure through a transport-level success, and this
crate treats it as exactly that — a real failure, not a false "accepted."

`transactionId` — "MADAPI generated Id to include for tracing requests"
— is what [`SubmitAck::provider_ref`] carries.

# `provider_ref_alt` is genuinely used here, the opposite of Orange

`sms-provider-orange-cm`'s own module doc records, at length, why
`provider_ref_alt` is always `None` for that adapter: Orange's real
submit request has no caller-supplied correlation field at all, so the
only value ever available to correlate a later DLR is Orange's own
`resource_id`, reported both at submit time (`resourceURL`) and DLR time
(`callbackData`) — one value, one column, `providerMessageRef` alone.

MADAPI is structurally different, and it is worth stating plainly rather
than leaving it to be re-derived: `outboundSMSMessageRequest` carries
`clientCorrelatorId` — "It uniquely identifies the request", explicitly
caller-supplied — and MADAPI's own DLR
(`DeliveryNotificationRequest.clientCorrelatorId`, "Message ID for which
the Derivery [sic] report is being sent") echoes that *same*
caller-supplied token back, not MADAPI's own `transactionId`. So this
adapter has two independent, genuinely useful correlation values:
MADAPI's own `transactionId` (`provider_ref`, `Message.providerMessageRef`)
and this system's own `Message.id` as sent in `clientCorrelatorId`
(`provider_ref_alt`, `Message.providerMessageRefAlt`) — and
`backends/crates/sms-api/src/dlr.rs` already matches a DLR against
either column, so both are checked with no code change needed there.
[`MtnProvider::submit`] sends `req.reference` (already `Message.id`,
per [`SubmitRequest::reference`]'s own doc) as `clientCorrelatorId` and
stores it as `provider_ref_alt` for exactly this reason.

**`clientCorrelatorId` has a documented `maxLength: 36`.** `Message.id`
is a `Cuid` — `cs_cuid()` emits 23 characters (`AGENTS.md`'s own
"Milestone 0" note) — so this never trips in practice. [`MtnProvider::submit`]
still checks it explicitly and refuses to submit rather than silently
truncating a value MADAPI would then echo back on a DLR that could never
correlate against anything: a silently truncated correlator is exactly
the kind of "shipped looking correct, wasn't" bug this codebase's own
history is full of. Mapped to [`ProviderError::Permanent`] — nothing is
wrong with the caller's `Message`, only with this specific provider's
own length limit on it, so a different route/provider for the same
message is worth trying.

# `Capabilities` stays contract-driven, unchanged in shape from before

This crate's original structural point — `tps_ceiling`,
`cost_per_segment_xaf`, and `supports_alphanumeric_sender` are
negotiated per-contract commercial terms with no public number MADAPI's
own docs give, so [`MtnConfig`] carries them as caller-supplied fields
and [`MtnProvider::capabilities`] reads them back rather than returning
a compiled-in constant — is unaffected by the aggregator-to-direct-API
rewrite and stays exactly as it was. See
`sms-provider-orange-cm/src/lib.rs`'s own `capabilities()` for the
contrasting case (Orange's own self-service, fixed 5 TPS/known-pricing
product), and `AGENTS.md`'s "Milestone 5" section for why this
difference is structural, not cosmetic.

# `subscribe_delivery_reports` — the operationally real difference this rewrite unlocks

Orange's own DLR endpoint is whitelisted by a manual support ticket —
§9.2's own stated external constraint, unautomatable by construction.
**MADAPI's `POST /messages/sms/subscription` registers a
`callbackUrl`/`deliveryReportUrl` through the API itself**, which means
this, unlike Orange's, genuinely can be automated.
[`MtnProvider::subscribe_delivery_reports`] is that call — an inherent
method, not part of [`SmsProvider`] (no other adapter has anything like
this concept, and forcing one onto the trait for MADAPI's sake alone
would be exactly the kind of adapter-specific leakage `sms-provider`'s
own module doc warns against). Nothing in this codebase calls it yet;
see the method's own doc for the intended caller (a future
`sms-gateway` subcommand, the same operator-action shape
`rotate-signing-key`/`provision-user` already use) and the one
simplifying decision its own doc names (`callbackUrl` and
`deliveryReportUrl` set to the same value, since this deployment has no
Mobile-Originating message handling to route a distinct `callbackUrl`
into).
