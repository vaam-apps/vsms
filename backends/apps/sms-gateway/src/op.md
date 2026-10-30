Mounting the OP's HTTP surface. #20.

Hand-wired to exactly the three routes a `client_credentials` +
`private_key_jwt` deployment needs — `/jwks.json`,
`/.well-known/openid-configuration`, `/token` — rather than
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
  `private_key_jwt` (#18). [`discovery_handler`] below calls it.

**The signing key and JWKS are live-refreshed, not a startup
snapshot.** Found in review (#97): the first version of this module
captured both once at construction, so `rotate-signing-key` run against
an already-running server updated the database but not the process —
new tokens kept signing with the old key indefinitely, and `/jwks.json`
never gained the new one, until a restart. That defeats the point of
rotation (a suspected-compromised key would keep signing) and the
entire 30-minute overlap window (`op.signing_key.rotation_overlap_secs`) was
only ever exercised at process start, never on a live server.
[`spawn_key_refresh`] closes this: a background poll reloads and
atomically swaps both.

# Restored by VSA-3a, and mounted only when `machine_tokens` is served

This module is `29f93c4^`'s, minus the console's half: no `/authorize`, no login,
no `vsms_sso_login_endpoint` in the discovery document. `serve` mounts [`router`]
**only** when `AuthMethods::machine_tokens_served` (the method permitted **and**
the `op:` block present); otherwise `/token`, `/jwks.json` and
`/.well-known/openid-configuration` are simply not routes, and answer 404 like
any other path (`tests/machine_tokens_gate.rs`). The routes sit **outside** the
envelope layer, as before: a `/token` caller has no key to sign with yet, it is
asking for a token. A token this module mints is not yet accepted by the API
(VSA-3b adds the bearer realm); until then it is minted and verifiable against
`/jwks.json` and nothing else.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
