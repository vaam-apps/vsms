Mounting the OP's HTTP surface. #20.

Hand-wired to exactly the three routes the provider needs — `/jwks.json`,
`/.well-known/openid-configuration`, `/token` (plus `POST /login`, `login.rs`,
when `human_login` is served) — rather than
`authkestra_axum::op::OpExt::op_axum_router()`, which mounts
`/authorize`, `/device_authorization`, `/userinfo` and `/device/verify`
too. Those need `SessionStore`/`SessionConfig` wiring for flows nothing
in this deployment uses yet (see `sms_auth::op`'s own module doc); this
keeps the served surface matching what's actually implemented instead
of exposing routes that would always fail.

Two places this deviates from the crate's own handlers, both because the
ready-made handler has no hook for what this deployment needs to do
differently:

- `authkestra_axum::op::axum_jwks_handler` publishes exactly one key
  (whatever single `Arc<TokenManager>` the state carries) — it cannot
  serve an overlap-window JWKS with both the active and a still-valid
  previous key. [`jwks_handler`] below builds the response from the full
  key list `sms_auth::op::load_signing_keys` already computed instead.
- `authkestra_axum::op::axum_discovery_handler` builds the discovery
  document straight from `OidcDiscovery::from_config`, with no way to
  chain `.with_private_key_jwt()` onto the result — so a spec-compliant
  client consulting discovery would never learn this OP accepts
  `private_key_jwt` (#18) - and it adds an `authorization_endpoint` for any
  provider that lists `authorization_code`, which here would point at a route
  that is never mounted. [`discovery_document`] chains the one on and drops the
  other (VSA-4a).

**The signing key and JWKS are live-refreshed, not a startup
snapshot.** Found in review (#97): the first version of this module
captured both once at construction, so `rotate-signing-key` run against
an already-running server updated the database but not the process —
new tokens kept signing with the old key indefinitely, and `/jwks.json`
never gained the new one, until a restart. That defeats the point of
rotation (a suspected-compromised key would keep signing) and the
entire 30-minute overlap window (`op.signing_key.rotation_overlap_secs`) was
only ever exercised at process start, never on a live server.
[`spawn_key_refresh`] closes this: a background poll reloads both and
swaps them in two steps, **the key set before the signing key**
(`Keyring::publish`; the two sit under separate locks, so the swap is not
atomic and the order is the guarantee). Between the steps `/jwks.json` already
advertises the new `kid` while tokens are still signed with the old one, which
every verifier can check; the other order had a window in which a token carried a
`kid` the JWKS did not list yet (inherited from `29f93c4^`).

# Restored by VSA-3a and VSA-4a, and mounted only when a method that rides it is served

This module is `29f93c4^`'s, minus `/authorize` (never mounted again: the console
reaches `handle_authorize` through `POST /login`). The SSO discovery member
`vsms_sso_login_endpoint` is back (VSA-4c): `discovery_document` adds it, the absolute
`<op.issuer>/login/sso/start` URL, exactly when `human_login.sso` is served and
`serve` handed the state the endpoint (`OpState::with_sso_login_endpoint`) - the console
learns from the one authority that knows both *whether* to offer the button and *where*
to send the browser. `serve` mounts [`router`] **only** when
`AuthMethods::op_served` (`machine_tokens` served, or `human_login` served: the
method permitted, its block present, and `op:` present); otherwise `/token`,
`/jwks.json` and `/.well-known/openid-configuration` are simply not routes, and
answer 404 like any other path (`tests/machine_tokens_live_postgres.rs`,
`with_the_method_off_nothing_is_mounted_and_the_commands_refuse`). The routes sit **outside** the
envelope layer, as before: a `/token` caller has no key to sign with yet, it is
asking for a token. A token this module mints is accepted by the API as a bearer
(VSA-3b): `op_bearer` verifies it against the keyring this module publishes
`/jwks.json` from, and `sms_api::bearer` turns it into a caller. The routes
stay outside both doors, because they are how a caller gets a token.

# The `/token` route's two layers (VSA-4a)

The grant gate (`grant_gate.rs`) is **outermost**, the bucket
(`token_rate_limit.rs`: per `client_id`, and per code for the `authorization_code`
grant) inside it: a request for a grant this deployment does not serve is refused
before it takes a token from anyone's bucket. [`OpState::store`]
and [`OpState::config`] are what `login.rs` feeds `handle_authorize`; the store's
clone shares the authorization-code `MemoryStore` with the `/token` handler's own,
which is what lets a code issued by `POST /login` be redeemed at `/token`.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
