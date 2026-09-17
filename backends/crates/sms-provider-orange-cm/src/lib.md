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

**The submit body and the DLR callback shape are both grounded in
Orange's own developer documentation now**
(<https://developer.orange.com/apis/sms/getting-started>), not the wider
GSMA `OneAPI` family this crate used to reason from by inference. The
submit body (`OutboundSmsMessageBody`) matches the docs' own "Getting
Started" sample exactly: `address`/`senderAddress`/`senderName`/
`outboundSMSTextMessage`, nothing else — there used to be a fifth field,
`receiptRequest` (`notifyURL`/`callbackData`), sending a caller-chosen
correlation token Orange's real docs never described; see
`OutboundSmsMessageBody`'s own doc comment for why it's gone and what
that costs. The DLR callback shape ([`dlr`]) is grounded the same way —
see that module's own doc for the full correlation story and what's
still genuinely unverified (nothing has been received from a live
Orange sandbox yet).

The connect-vs-read transport classification and the provider-agnostic
half of the HTTP-status → `ProviderError` mapping live in
`sms-provider-http` now, not in this crate — this crate's own
`classify_transport_error`/`classify_submit_error` are thin wrappers
supplying Orange's own provider noun and rate-limit delay. See that
crate's module doc for why the DRY-up landed in a sibling crate rather
than a module here or in `sms-provider` itself.
