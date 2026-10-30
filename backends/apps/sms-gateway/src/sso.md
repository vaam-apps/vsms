The two raw routes of the federated (SSO) sign-in, and the task that cleans up after
them (`GET /login/sso/start`, `GET /login/sso/callback`), VSA-4c.

Not CrateStack-routed, for the structural reason `login.rs` isn't: they run before any
bearer token exists. `serve` merges them beside `POST /login`, outside the envelope and
the bearer door, **only when `human_login.sso` is served** (otherwise they are the
router's 404, answered exactly as a path that never existed). The protocol and the
decisions are in `sms_auth::sso`'s module doc, and the operator's side is
`docs/runbooks/sso.adoc`. This file is the HTTP seam only.

# What `start` checks, in order

```text
bucket -> every parameter present and bounded -> the console's client_id -> the cheap checks -> dry-run handle_authorize -> row, then upstream
```

1. **The bucket**, `sso:start`, before anything is read or written (below).
2. **Every parameter**: the eight OAuth parameters of the console's request, each present,
   non-empty and at most 1024 bytes (`400`). `nonce`, `state` and `scope` are required here
   although the library treats them as optional: a transaction stores the console's own
   state to hand back, and an SSO sign-in with no nonce is one the console cannot bind.
3. **`client_id` is the console's** (`human_login.console_client.client_id`), else `400`. The
   OP knows other clients, and the library alone would let one with the code grant and a
   matching redirect through; only the console is served by this route.
4. **The same cheap checks `POST /login` makes** (`console_authorize.rs`): `S256` only, a
   43 to 128 character base64url challenge, a registered `redirect_uri`. A request that
   can never succeed stores nothing.
5. **A dry run of the real `handle_authorize`.** `authkestra_op::handlers::authorize::handle_authorize`
   validates the console's request (client registered, `redirect_uri` an exact match,
   `response_type`, scope, S256 PKCE) **and then issues the authorization code** - and it
   needs an `Identity` to do it. At `start` there is no identity yet, so `start` runs the
   *same, unmodified* function against a throwaway store
   (`sms_auth::op::op_store` builds a fresh in-memory code slot per call, which is not the
   one `/token` redeems) with a placeholder identity, and throws the code away. Nothing
   about client, scope or PKCE checking is re-implemented here. **Any refusal is a plain
   `400`, never forwarded**: the library answers a request it refuses with a redirect to
   the client carrying `error=`, and the first SSO broker passed that to the browser. VSA-4a's
   rule for the password path - the console is never handed an error redirect as though it
   were a sign-in - holds here too.
6. **Only now a row** (`SsoTransaction`: the hashed state, the nonce, the PKCE verifier and
   the console's request as JSON) **and a `302` to the identity provider.** A failure to
   reach the provider or the database is the console's `error=temporarily_unavailable`.

# What `callback` does

It consumes the row (a delete: of two concurrent callbacks for one `state`, exactly one wins),
reads the console's request back, **checks its `redirect_uri` is still one the console client is
registered with** (the list can change between `start` and the callback, and an error redirect is
still a redirect: otherwise a plain `400`), and runs `sms_auth::sso::flow::complete`. The upstream
`code` may be up to 4096 bytes where every other parameter is capped at 1024: it is the one value
the identity provider, not the console, makes, some providers' codes are themselves long signed
tokens, and it is never stored. A verified,
admitted sign-in then runs the real `handle_authorize` with
`Identity { provider_id: "sms-console", external_id: <User.id>, email, username }`, the shape
`POST /login` issues, so VSA-4b's person realm and its owner and admin cap apply to the
token unchanged. The browser gets a `302` to the console's `redirect_uri` with the code
(a navigation, unlike `POST /login`, which answers the console's server in JSON). An error
redirect from the library at this point is a `400` too, not a sign-in.

**`lastLoginAt` is stamped last**, once the code is issued (`sms_auth::sso::store::stamp_last_login`),
exactly as `POST /login` does: a verified token whose authorization request the library then
refuses signed nobody in. A first sign-in creates the account with no stamp for the same reason.

# What the console must do with this (VSA-5)

**The upstream `state` is bound to a Postgres row, not to the browser.** Anyone who holds a
live `state` can complete the callback in their own browser and be sent to the console's
`redirect_uri` with a code that is *theirs*: login CSRF, the OAuth 2.0 threat model's
classic. The gateway cannot see the console's cookie, so the binding is the console's:

- it starts a sign-in by setting its own transaction cookie (`vsms_oidc_txn`) holding the
  `state` it passed to `start`, the PKCE verifier and the `nonce`, and sets it **before**
  sending the browser to `/login/sso/start`;
- on its callback it **refuses a `state` that differs from the cookie's**, and never redeems
  a code whose `state` it did not start;
- it redeems the code at `/token` with that verifier, once, and ends the cookie.

`/login/sso/start` is, in effect, a browser `GET` authorization endpoint: the console
navigates the browser to it with the eight OAuth parameters in the query string, exactly as
it would to any authorization server, and gets a `302` back to the console's `redirect_uri`
(`?code=` and `state`, or `?error=`). It is not the JSON `POST /login`, and a console written
against that one must not call it from a server.

# What the browser is told

Every refusal after a transaction was consumed is one redirect: the console's own
`redirect_uri` with `error=access_denied` (and the console's `state`) - the standard OAuth
error redirect, so the console's existing callback route is the single place that turns it
into a message. Operational failures (the `IdP` or the database is down) are
`error=temporarily_unavailable`. No detail useful for enumeration ever leaves the process:
the precise reason goes to the log (`sms_gateway=info`, a fixed label such as
`not_in_required_group` or `email_unverified`), and never a token, a code, a secret, or a claim value.

A callback whose `state` matches no live transaction (unknown, replayed, expired) has
nowhere trustworthy to redirect, so it gets a plain `400`.

# One bucket per route, and no bucket per source

`human_login.sso.rate_limit` is one in-process bucket for `start` and one for `callback`,
both global. There is no per-source bucket, for the reason `login.md` gives: this process
sees one peer, the proxy, for everybody. So one source can empty a route's bucket for
everyone, and the answer is the edge: `deploy/Caddyfile`'s `sso_per_source` zone bounds a
single source first. Password sign-in is a different route with different buckets and is
unaffected. A store that cannot answer fails closed (`503`).

Rows are deleted when consumed and, if never consumed, by [`spawn_transaction_reaper`]
(the first sweep at boot, then every minute) after five minutes. `serve` spawns it only
when SSO is served.

# At boot

`serve` reads the configuration (the loader has refused everything it can), builds the
provider without contacting the identity provider (a dead Keycloak must not stop the
gateway: password sign-in is the break-glass path), checks that the `Role` row named by
`human_login.sso.jit_role` exists (a typo would otherwise be the first person's failed
sign-in), spawns the reaper, advertises `vsms_sso_login_endpoint` in the discovery
document, and logs the redirect URI to register at the provider.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
