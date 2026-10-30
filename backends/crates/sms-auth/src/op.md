Standing up the OP itself: RS256 key management with an overlap-window
rotation, [`CompositeOpStore`] assembly, and [`OpConfig`]. #20, restored by
VSA-3a.

Two grants ride this provider, each behind its own switch ([`ServedGrants`]):
`client_credentials` with `private_key_jwt` for machines (`machine_tokens`,
VSA-3a) and `authorization_code` with PKCE for the console's sign-in
(`human_login`, VSA-4a). [`op_config`] advertises exactly the ones served
([`ServedGrants::grant_types`]), and `sms-gateway`'s grant gate in front of
`/token` refuses every other with `unsupported_grant_type`, so a deployment that
serves one cannot be asked for the other.

**There is no refresh-token grant, ever.** `authkestra`'s `client_credentials`
handler returns `refresh_token: None` unconditionally, so a machine client never
held one. The console's `29f93c4^` registration listed `refresh_token` and its
sign-in could mint a 30-day credential for the asking; it does not any more
(owner ruling, 2026-10-09: *"Never ever should refresh tokens be used in this
system."*). Three independent refusals hold that: the console client is
registered for `authorization_code` alone ([`crate::console_client`] refuses to
let `serve` start on a row that carries more); the grant gate answers a
`refresh_token` request `unsupported_grant_type`; and the store's refresh slot is
[`RefusingRefreshStore`], which cannot hold one. The function and type names that
kept the word "machine" are renamed: read [`OpStore`] and [`op_config`] as "the
OP's one store and config".

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
  `with_client_assertion_store` is the one real builder method. The slots are a
  `MemoryStore` for codes, [`RefusingRefreshStore`] and an inert `MemoryStore` for
  device codes (see [`OpStore`]).
- **`handle_authorize` does not read `grant_types_supported`.** It checks the
  *client's* registration (redirect URI, scope, `response_type=code`, the client's
  own grants, S256 PKCE), never the OP's list, which is why the console client's
  row and the grant gate carry the rule and why `POST /login` accepts only the
  configured `client_id`.
- **The authorization code is single-use and in memory.** `consume_code` removes
  it atomically; a second `/token` with the same code finds nothing. A code lives
  [`AUTHORIZATION_CODE_TTL_SECS`] seconds, in the process that issued it: one
  replica (`OpStore`'s doc has the decision).
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
