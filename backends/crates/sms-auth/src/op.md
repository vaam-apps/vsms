Standing up the OP itself: RS256 key management with an overlap-window
rotation, [`CompositeOpStore`] assembly, and [`OpConfig`]. #20, restored by
VSA-3a.

Scoped to `client_credentials` + `private_key_jwt` only, as it was before the
console login (#194) widened it and #446 deleted both. [`machine_only_config`]
advertises that one grant ([`GRANT_TYPES_SUPPORTED`]). In particular **there is
no refresh-token grant**: `authkestra-op`'s `client_credentials` handler returns
`refresh_token: None` unconditionally, so a machine client never held one, and
the `refresh_token` and `authorization_code` entries `29f93c4^` listed were the
console's. They come back, if at all, with the human login (VSA-4), and only
while a method that needs them is served (owner ruling R5, question 8). The
function and type names keep the word "machine" for the same reason they did:
read it as "the OP's one store/config".

# API reality check

Verified against the vendored `authkestra-op`, `authkestra-engine` and `authkestra-axum` 0.8.0 source:

- **No `SigningKey`/rotation type exists.** [`TokenManager::new_asymmetric`]
  holds exactly one RS256 key. An overlap window means running one
  [`TokenManager`] per still-valid [`schema::OauthSigningKey`] row and
  merging their [`TokenManager::public_jwk`]s ourselves: see
  [`load_signing_keys`].
- **`authkestra-axum`'s own `axum_jwks_handler` publishes exactly one key**
  (whichever single `Arc<TokenManager>` the axum state carries), so it cannot
  serve an overlap-window JWKS on its own. `sms-gateway` builds its `/jwks.json`
  response from [`load_signing_keys`]'s full key list instead.
- **`CompositeOpStore::new` takes 4 positional stores** (client,
  authorization-code, refresh-token, device-code), not `with_*` builders;
  `with_client_assertion_store` is the one real builder method. The three grant
  slots are inert `MemoryStore`s (see [`MachineOnlyOpStore`]).
- **`state_encryption_key`/`SessionConfig` is not part of the OP at all.** It
  belongs to `authkestra_engine`'s *relying-party* flow (this system consuming an
  external identity provider), which nothing here does.
- **No `Engine` is ever constructed.** `sms-gateway`'s own `op.rs` hand-wires
  `axum_token_handler` directly; it only needs `Arc<dyn CloneableOpStore>`,
  `Arc<TokenManager>` and `OpConfig`, all via `FromRef`.

# How the stores reach the tables

`OauthClient`, `OauthSigningKey` and `ClientAssertion` carry
`@@internal("all")` (VSA-2): no generated REST route, RPC arm or client stub.
That attribute removes **routes**, not the server-side delegates, so
`db.oauth_client()`, `db.oauth_signing_key()` and `db.client_assertion()` exist and
every call here is a delegate call under the internal `system` context
(`sms_api::system_context`), whose policy is the only one these models admit. No
`Access`-style bypass, no raw `sqlx`, and no change to the schema. The private
signing key is therefore readable in exactly one place, [`load_signing_keys`],
and reachable over HTTP by nothing: `sms-api`'s `provider_models_are_unreachable`
test holds that.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
