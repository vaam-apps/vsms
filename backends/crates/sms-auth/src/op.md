Standing up the OP itself: RS256 key management with an overlap-window
rotation, [`CompositeOpStore`] assembly, and [`OpConfig`]. #20.

**Was** scoped to `client_credentials` + `private_key_jwt` only — #97/98
cut the authorization-code/human-login path because no admin console
existed yet to need it. #194 closes that cut: [`machine_only_config`]
now advertises `authorization_code`/`refresh_token` alongside
`client_credentials`, and the `CompositeOpStore` this module already
assembled turns out to need **no change at all** for it — see the
doc on [`machine_only_store`] for why the authorization-code/refresh-
token slots this module always wired to `MemoryStore` were already a
real, working implementation, just never exercised. The function/type
names below keep the word "machine" for now rather than a
repo-wide rename touching every test file that already imports them;
read it as "the OP's one store/config", not a claim about who can use
it — `backends/crates/sms-auth/src/login.rs` is #194's own human half.

# API reality check (verified against vendored `authkestra-op`/
`authkestra-engine`/`authkestra-axum` 0.3.2 source, not the design doc,
which was written for 0.2.3)

- **No `SigningKey`/rotation type exists.** [`TokenManager::new_asymmetric`]
  holds exactly one RS256 key. An overlap window means running one
  [`TokenManager`] per still-valid [`schema::OauthSigningKey`] row and
  merging their [`TokenManager::public_jwk`]s ourselves — see
  [`load_signing_keys`].
- **`authkestra-axum`'s own `axum_jwks_handler` publishes exactly one
  key** — whichever single `Arc<TokenManager>` the axum state carries —
  so it cannot serve an overlap-window JWKS on its own. `sms-gateway`
  builds its `/jwks.json` response directly from
  [`load_signing_keys`]'s full key list instead of using that handler.
- **`CompositeOpStore::new` takes 4 positional stores** (client,
  authorization-code, refresh-token, device-code), not `with_*`
  builders — `with_client_assertion_store` is the one real builder
  method. The authorization-code/refresh-token/device-code slots are
  wired to [`authkestra_engine::store::memory::MemoryStore`] — genuinely
  inert placeholders, since nothing calls `/authorize` on this
  client-credentials-only deployment, not a corner cut with anything
  real behind it to lose.
- **`state_encryption_key`/`SessionConfig` is not part of the OP at
  all** — it belongs to `authkestra_engine`'s *relying-party* flow
  (this system consuming an external `IdP`), which nothing here does.
- **No `Engine` is ever constructed.** The obvious-looking path —
  `Engine::builder().session_store(...).token_manager(...).build()`,
  then handing that to `Op::builder()` — turned out to be unnecessary:
  `backends/apps/sms-gateway`'s own `op.rs` hand-wires `axum_token_handler`/
  `axum_discovery_handler` directly, and those only need `Arc<dyn
  OpStore>` + `Arc<TokenManager>` + `OpConfig` via `FromRef`, never a
  full `Op`/`Engine`. An earlier version of this module built the
  `Engine` anyway, unused — removed in review (#97) rather than kept
  "for a future caller."
