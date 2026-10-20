The two raw routes of the federated (SSO) sign-in, and the task that cleans
up after them (`GET /login/sso/start`, `GET /login/sso/callback`).

Not CrateStack-routed, for the structural reason `login.rs` isn't: they run
before any bearer token exists. The protocol and the decisions are in
`sms_auth::sso`'s module doc; this file is the HTTP seam only.

# What `start` does that `handle_authorize` cannot

`authkestra_op::handlers::authorize::handle_authorize` validates the
console's authorize request (client registered, `redirect_uri` an exact
match, `response_type`, scope, S256 PKCE) **and then issues the
authorization code** - and it needs an `Identity` to do it. At `start` there
is no identity yet, so `start` runs the *same, unmodified* function against a
throwaway store (`sms_auth::op::machine_only_store` builds a fresh in-memory
code slot per call) with a placeholder identity, and throws the code away.
Nothing about client, redirect or PKCE checking is re-implemented here; the
real code is issued only at `callback`, by a second, real call with the real
`Identity`, which re-validates everything again.

# What the browser is told

Every refusal after a transaction was consumed is one redirect: the
console's own `redirect_uri` with `error=access_denied` (and the console's
`state`) - the standard OAuth error redirect, so the console's existing
callback route is the single place that turns it into a message. Operational
failures (the `IdP` or the database is down) are `error=temporarily_unavailable`.
No detail useful for enumeration ever leaves the process; the precise reason
goes to the log, and never a token, a code, a secret, or a claim value.

A callback whose `state` matches no live transaction (unknown, replayed,
expired) has nowhere trustworthy to redirect, so it gets a plain `400`.

# Abuse limits

`start` writes a row per request and `callback` reads one, both before any
authentication, so both sit behind a small in-process token bucket
(a backstop) and, in production, behind per-source zones in
`deploy/Caddyfile`. Rows are deleted when consumed and, if never consumed, by
[`spawn_transaction_reaper`] after five minutes.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
