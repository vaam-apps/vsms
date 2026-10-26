Mounting the raw `POST /login` route (#194).

Not CrateStack-routed, for the same structural reason `dlr.rs`'s own
route isn't: a login attempt carries no signed envelope for the
envelope layer to verify (the layer seals every generated route) — proving who
the caller is is the entire point of this route.

# What this is, precisely

`authkestra_op::handlers::authorize::handle_authorize` needs an already-
established `authkestra_engine::auth::state::Identity` before it will
issue an authorization code — it has no login form of its own, and
§4.3/#194 chose local password auth over federating to an external `IdP`
(see `sms_auth::login`'s own module doc for the full reasoning) — and
federation was added afterwards as a *second* way to establish the
`Identity` (`sso.rs` in this directory, `sms_auth::sso`), not a
replacement: this route stays as the break-glass path. So this
route collapses what a spec-shaped OIDC deployment would split across a
`GET /authorize` redirect and a separately-hosted login page into one
step: the caller (`admin`'s own `/login` page and `/api/auth/login`
route handler — never a browser talking to this route directly) submits
credentials *and* the full `AuthorizeRequest` shape in one POST, and
this handler does both jobs — verify the password, then run the real
`handle_authorize` — in one call.

**Every OAuth2/OIDC security property `#194` requires is still enforced
by the real library code, not reimplemented here:** PKCE (S256 only),
`redirect_uri` exact-match, `response_type`, grant-type admission, and
authorization-code issuance are all `handle_authorize`'s own logic
(`authkestra_op::handlers::authorize`), unmodified. This route's own
job is exactly one thing `handle_authorize` cannot do for itself:
decide *who the caller is* before calling it.

`state`/`nonce` pass through untouched to `handle_authorize`/the
resulting authorization code — this route neither generates nor
verifies either; that's `admin`'s own job, both ends (see
`frontends/apps/admin/app/login/page.tsx` for where they're minted and
`frontends/apps/admin/app/api/auth/callback/route.ts` for where they're checked against
the redirect this route ultimately returns).

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
