Mounting the raw DLR webhook route. #34, extended for a second adapter in
#61's MTN wiring.

`POST /dlr/{providerKey}` — not CrateStack-routed (see
`sms_api::dlr`'s own module doc for why: a provider webhook carries no
bearer token to validate against `GatewayAuth`). The one access control
this route has is the path segment matching one of the configured
providers' own `key()` — everything past that is §9.2's own stated
external constraint: Orange will only call a webhook on HTTPS 443 with a
CA-signed cert, whitelisted per a manual support ticket. No app-level
signature verification is implemented — `RawCallback` already carries
the exact, unmodified bytes a future one would need, but no provider's
real signature scheme is documented yet to verify against (Orange's
own DLR shape is itself unverified against a live sandbox — see
`sms-provider-orange-cm`'s own `dlr` module; MTN's is an outright
invented placeholder — see `sms-provider-mtn`'s own module doc).

`{providerKey}` is looked up in a map keyed by each configured adapter's
own `SmsProvider::key()` (`DlrProvider`, this module), not compared
against a single hardcoded provider — before #61's wiring, this route
held exactly one adapter and 404'd every key that wasn't Orange's,
including a key that genuinely belonged to a *second*, simply
unconfigured, provider. With N adapters configured (today: Orange,
`orange_cm`, and optionally MTN, `mtn_aggregator` — see
`commands::serve::build_dlr_router`), a 404 means "no adapter is
configured under this key," which is now the only thing it can mean.

The response status on a successful ingest is `200 OK`, not `202
Accepted` — Orange's own published contract
(<https://developer.orange.com/apis/sms/getting-started>, §4) states the
webhook receiver "must return an HTTP 200 OK in order to acknowledge the
receipt" and shows exactly that as its worked example. `docs/architecture.md`
§3.2's "return 202, never 200" rule governs `sendMessage`, this API's own
*outbound* send endpoint (the reasoning is "you have not sent anything
yet" — not applicable to a webhook acknowledging something a provider
says already happened); §3.2/§4.1/§6.2 all state this distinction
explicitly and record that this exact rule was once mis-cited the other
way around to justify `202` on this very route.
