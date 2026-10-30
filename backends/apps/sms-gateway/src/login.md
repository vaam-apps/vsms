`POST /login` (VSA-4a, restored from `29f93c4^`): authenticate a person and run
the real `handle_authorize` in one call.

Not CrateStack-routed, for the structural reason `dlr.rs` is not: the envelope
layer seals every generated route and a sign-in carries no signed envelope,
because proving who the caller is is the entire point. `serve` merges it beside
`/token`, outside the envelope and the bearer door, **only when `human_login` is
served** (otherwise it is the router's 404).

# What this is, precisely

`authkestra_op::handlers::authorize::handle_authorize` needs an established
`Identity` before it issues a code, and has no login form of its own. A spec-shaped
deployment splits that across a `GET /authorize` redirect and a hosted login page;
this one collapses it: the console's server posts credentials **and** the whole
authorization request, this handler verifies the password, then runs the library's
own `handle_authorize`. `GET /authorize` is never mounted, and the discovery
document advertises no `authorization_endpoint`.

Every OAuth security property is still the library's, unmodified: PKCE with S256
only (any other method, or none, is refused), exact `redirect_uri` match, the
client's own scope and grant admission, `response_type=code`, and the code itself,
single-use, sixty seconds, in memory. This route decides one thing the library
cannot: who the caller is.

# The order, and why

```text
parse body -> client_id is the console's -> rate limit -> Argon2 -> handle_authorize
```

1. **A body that does not parse is `400 invalid_request`**, a field short, an
   unknown field, or more than 16 KiB.
2. **Only the configured `client_id` is accepted** (`human_login.console_client`).
   Any other is the same `401 invalid_credentials` as a wrong password, before the
   limiter, so a caller with another id cannot spend anyone's email bucket and
   learns nothing about what ids exist.
3. **The limiter runs before any password is hashed** (`login_rate_limit`): one
   token from the **source address** bucket, then one from the **email** bucket
   (trimmed, lowercased, capped). That order is the point of the limiter: it bounds
   how many Argon2 verifications a caller can make this process run. A throttled
   request is `429` with `Retry-After`; a limiter store that cannot answer is `503`
   and hashes nothing.
4. **One generic `401 {"error": "invalid_credentials"}` for every refusal of the
   person**: an unknown account, a wrong password, an inactive or deleted account,
   an account with no password, a role that does not exist or is reserved. An
   outage is `503 {"error": "unavailable"}`, not a 401, so a database that is down
   is not read as a wrong password.
5. **`handle_authorize`'s answer is mapped, not forwarded.** The library answers a
   request it refused (a challenge method other than S256, a scope the console may
   not hold) with a *redirect carrying `error=`*; that is `400 invalid_request`
   here, because the console must never be handed an error redirect as though it
   were a sign-in. A `redirect_uri` the console is not registered with is a direct
   error, never redirected to.

The source address is the real TCP peer (`ConnectInfo`), the choice
`sms_api::router`'s own source limiter documents; a forwarded-for header is
attacker-controlled unless a proxy is trusted, and none is configured.

# Wire shape

Request (JSON, `snake_case`, unknown fields refused): `email`, `password`,
`client_id`, `redirect_uri`, `response_type`, `scope`, `state`, `code_challenge`,
`code_challenge_method`, `nonce`. Response `200 {"redirect": "<redirect_uri>?code=…&state=…"}`.
Every response is `Cache-Control: no-store`. (`29f93c4^` read camelCase for a
console that no longer exists; the repository's rule is that a shape it owns is
`snake_case`, and these are the OAuth parameter names themselves.) The console has
no sign-in screen on this release: VSA-5 builds it against this.

# What the code carries

An `Identity` of provider `sms-console`, `external_id` the `User.id`, the email
and display name. No role, no permission. The API resolves those from rows per
request (VSA-4b), so a deactivation takes effect within one cache window and a code
cannot outlive a demotion.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
