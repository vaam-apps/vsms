Mounting the raw DLR webhook route. #34.

`POST /dlr/{providerKey}` — not CrateStack-routed (see
`sms_api::dlr`'s own module doc for why: a provider webhook carries no
bearer token to validate against `GatewayAuth`). The one access control
this route has is the path segment matching the configured provider's
own `key()` — everything past that is §9.2's own stated external
constraint: Orange will only call a webhook on HTTPS 443 with a
CA-signed cert, whitelisted per a manual support ticket. No app-level
signature verification is implemented — `RawCallback` already carries
the exact, unmodified bytes a future one would need, but no provider's
real signature scheme is documented yet to verify against (Orange's
own DLR shape is itself unverified against a live sandbox — see
`sms-provider-orange-cm`'s own `dlr` module).
