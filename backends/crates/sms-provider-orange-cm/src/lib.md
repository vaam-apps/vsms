[`SmsProvider`] for Orange Cameroon's SMS Cameroon 2.0 / on-net HTTP API.
§6.2 of the design doc — "build this first": genuinely self-service,
start sending within the hour of registering on the Orange developer
portal.

**Ceilings that shape this crate, not just document it:** a hard 5 TPS
cap and a 100k FCFA/day SIM cap ceiling throughput at roughly 5,000
SMS/day (#31). Nothing here enforces either — that is `dispatch`'s
`budget` parameter to the claim loop (§7.3, `backends/crates/sms-worker`), which
this adapter has no visibility into. [`OrangeCmProvider::capabilities`]
reports [`Capabilities::tps_ceiling`] as data for the caller to enforce,
not a limit this crate self-polices.

Two things in here are transcribed directly from §6.2 and verified only
by rereading the doc precisely, not against a live Orange sandbox (this
repo has no Orange Developer credentials): the OAuth token endpoint and
TTL handling ([`token`]), and the submit request/response shape below.
The submit body's `receiptRequest` (`notifyURL`/`callbackData`, #95) is
one step further still — §6.2 doesn't mention it at all; it's grounded
in the public `OneAPI` SMS Messaging REST binding this whole shape
belongs to, not this repo's own design doc. The DLR callback shape
([`dlr`]) is the same distance from §6.2 — see that module's doc.

The connect-vs-read transport classification and the provider-agnostic
half of the HTTP-status → `ProviderError` mapping live in
`sms-provider-http` now, not in this crate — this crate's own
`classify_transport_error`/`classify_submit_error` are thin wrappers
supplying Orange's own provider noun and rate-limit delay. See that
crate's module doc for why the DRY-up landed in a sibling crate rather
than a module here or in `sms-provider` itself.
