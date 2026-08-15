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
entire 30-minute overlap window (`sms_auth::op::ROTATION_OVERLAP`) was
only ever exercised at process start, never on a live server.
[`spawn_key_refresh`] closes this: a background poll reloads and
atomically swaps both.
