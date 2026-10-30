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
parse body -> client_id is the console's -> the cheap checks -> rate limit -> Argon2 -> handle_authorize -> stamp lastLoginAt
```

1. **A body that does not parse is `400 invalid_request`**, a field short, an
   unknown field, or more than 16 KiB.
2. **Only the configured `client_id` is accepted** (`human_login.console_client`).
   Any other is the same `401 invalid_credentials` as a wrong password, before the
   limiter, so a caller with another id cannot spend anyone's email bucket and
   learns nothing about what ids exist.
3. **What the request alone proves wrong is a `400 invalid_request`, before the
   limiter and before any Argon2 work**: a `code_challenge_method` other than
   `S256`, a `code_challenge` that is not 43 to 128 base64url characters (the
   library would accept an empty one), and a `redirect_uri` that is not in
   `human_login.console_client.redirect_uris`. None of them depends on who the
   caller is, so none is the person's `401` and none costs a bucket token: a
   request that can never succeed spends nothing. (The library still enforces all
   three; this is the same refusal made early, in the response shape the library's
   own would be mapped to.)
4. **The limiter runs before any password is hashed** (`login_rate_limit`): one
   token from the **email** bucket (trimmed, lowercased, capped), then one from the
   **aggregate** bucket. That order is the point: the aggregate bounds how many
   Argon2 verifications this process can be asked for, whoever asks, and the email
   bucket first means an address the limiter has already refused never reaches it
   (hammering one address spends that address's tokens and nobody else's). A
   throttled request is `429` with `Retry-After`; a limiter store that cannot
   answer is `503` and hashes nothing.
5. **One generic `401 {"error": "invalid_credentials"}` for every refusal of the
   person**: an unknown account, a wrong password, an inactive or deleted account,
   an account with no password, a role that does not exist. An
   outage is `503 {"error": "unavailable"}`, not a 401, so a database that is down
   is not read as a wrong password.
6. **`handle_authorize`'s answer is mapped, not forwarded.** The library answers a
   request it refused (a scope the console may not hold, a response type that is
   not `code`) with a *redirect carrying `error=`*; that is `400 invalid_request`
   here, because the console must never be handed an error redirect as though it
   were a sign-in. A `redirect_uri` the console is not registered with is a direct
   error, never redirected to.
7. **`lastLoginAt` is stamped last**, once a code has been issued
   (`sms_auth::login::stamp_last_login`), not when the password was right: a
   correct password whose authorization request the library then refuses signed
   nobody in.

# No bucket per source address

There is none, and that is a ruling of this file, not an omission. The only
address this process can see is the TCP peer. Behind Caddy it is Caddy; behind the
console's own server (which calls the gateway directly over the compose network
and does not pass through Caddy) it is that server; in both cases it is **the same
address for every person**. A per-source bucket sized for one person (the example
config's first draft was a burst of 5 refilling at 0.1 a second) is therefore a
global bucket, and one bad `/login` every ten seconds from anywhere would keep it
empty and every operator out. `X-Forwarded-For` is not the way out: the compose
stack has two internal callers of this router and no static address to pin a
trusted hop to (`sms_api::router`'s own source limiter documents the same), and a
header that is believed from anyone is chosen by the attacker.

What bounds a single source instead is the edge: `deploy/Caddyfile`'s
`auth_per_source` zone keys `/login` and `/token` on the real client address
(`{http.request.remote.host}`, an IPv6 `/64`), where that address is known. It
covers a caller who reaches the gateway's public origin. **A caller who reaches
`/login` through the console's server is not covered by it**: the console (VSA-5)
sees its visitor's address and has to bound it there, and an edge zone on the
admin origin belongs in that change, with the paths it adds.

# The two buckets, and what is left of the lockout

- **`per_email`** (the example: burst 5, one more try every 10 seconds) is what
  one account can be tried with. It is the part that remains a *targeted* lockout:
  anyone who knows an operator's email address can keep that one operator throttled
  by sending a bad password every ten seconds. It affects that address only (the
  test `one_address_hammering_the_sign_in_cannot_lock_other_operators_out` holds
  it to that), the person can still sign in with a paired console key, and it is the
  price of bounding a password guess per account. A bucket that counted only
  failures would remove it and is not available: the store can consume a token and
  not look at one.
- **`aggregate`** (the example: burst 10, one a second) is the Argon2 CPU budget. An
  Argon2id verification at the library's default cost (19 MiB, two passes, one
  lane) is tens of milliseconds of one core and 19 MiB of memory, against about
  a millisecond for a sealed API call; `sms_api::router`'s source limiter admits
  `RateLimitConfig::new(1200, 10.0)` of those cheap calls, so a tenth of its refill
  is the order of magnitude that keeps the hashing to a small fraction of a core
  (one a second is a few percent of one at 50 ms) and the burst to what a handful of
  operators signing in at once need (ten simultaneous hashes are about 190 MiB
  at their worst). It is also a bucket an attacker can empty: that takes more than
  one source per second, which the edge zone makes the work of at least six
  addresses. Distributed flooding of an unauthenticated password check can be
  slowed, not prevented, and when it happens the sign-in answers `429` while the
  paired-key door and the machine doors, which do no Argon2 work, are untouched.

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
