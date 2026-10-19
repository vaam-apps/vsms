A fake upstream `OpenID` Provider - a Keycloak stand-in for vsms's SSO tests.

`sms-gateway` is an OIDC *relying party* toward the staff identity provider
(`docs/architecture.md` section 4.3), and this repo's convention is a fake
for every external dependency (`sms-fake-orange` is the precedent). This is
that fake. It speaks just enough of the protocol for the gateway's broker to
run unmodified against it over real HTTP:

- `GET {issuer}/.well-known/openid-configuration` - discovery.
- `GET {issuer}/jwks.json` - the published signing keys.
- `GET {issuer}/authorize` - a **browserless** authorization endpoint: it
  validates the request the way a real `IdP` would (`client_id`,
  `response_type=code`, an S256 `code_challenge`), pops the next scripted
  [`LoginSpec`] and answers `302 redirect_uri?code=...&state=...` at once.
  A test plays the browser by following redirects by hand.
- `POST {issuer}/token` - `client_secret_basic` authentication, the code and
  the PKCE verifier checked, and an `id_token` signed per the scripted spec.

# Why not `wiremock`

`sms-fake-orange` is built on `wiremock` because Orange's contract is a
fixed request/response. This one is stateful in a way `wiremock` makes
awkward: the `nonce` and `state` the test sends must come back inside a token
signed *at exchange time*, an authorization code is single-use, and the PKCE
challenge seen at `/authorize` must be checked at `/token`. A small `axum`
app holding that state is simpler than a `wiremock` responder closure
fighting it.

# What it can get wrong on purpose

The reason it exists. A [`LoginSpec`] can script a wrong `nonce`, `aud`,
`iss` or `azp`, an expired token, a missing or unknown `kid`, a token signed
with a key the relying party has never seen, `alg: none`, `alg: HS256`, and
any `acr`/`amr`/`groups` combination - so every refusal in the broker's
verification path is exercised by a token that is *actually* malformed, not
by a unit test that asserts on a helper.

The keys are throwaway literals (see `keys.rs`); nothing here protects
anything.
