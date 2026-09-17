`OAuth2` `client_credentials` token acquisition and caching for MTN's
direct API (MADAPI), mirroring `sms-provider-orange-cm::token` almost
exactly — same 80%-of-lifetime refresh margin, same plain
`std::sync::RwLock` reasoning (no `.await` inside any critical section),
same no-in-flight-gate acceptance (see [`MtnProvider::access_token`]'s
own doc). Two differences from Orange's copy, both load-bearing and both
forced by the vendored Swagger, not stylistic:

- **The token URL is not a `base_url`-relative path.** Orange's is
  `{base_url}/oauth/v3/token` — a path under the same API tree. MTN's
  own `securityDefinitions.OAuth2.tokenUrl`
  (`mtn-sms-v3-swagger.yaml:19`) is a full, absolute URL,
  `https://api.mtn.com/v1/oauth/access_token/accesstoken?grant_type=client_credentials`
  — same host as the SMS API (`api.mtn.com`), but under `/v1/...`, a
  sibling of the SMS API's own `/v3/sms/...` `basePath`, not nested
  inside it. [`MtnProvider::token_url`] derives the scheme+host+port from
  `MtnConfig::base_url` rather than hardcoding `https://api.mtn.com` a
  second time, purely so a test can point both endpoints at one mock
  server the way this crate's own tests already do — production's real
  `base_url` is `https://api.mtn.com`, matching the Swagger's own `host`,
  so this changes nothing about what a real deployment actually calls.
- **`grant_type=client_credentials` is already on the token URL's query
  string**, so [`fetch`] does not also send a second, form-encoded copy
  of it in the request body the way Orange's own `fetch` does (Orange's
  `tokenUrl` carries no query string at all, so the form body is the
  only place `grant_type` can go). Sending both would very likely be
  harmless — most `OAuth2` server implementations read whichever one is
  present — but there is no reason to invent a second copy of a value
  the URL already carries, and every extra invented byte in this
  request is one more thing to get right against a spec this crate has
  never been run against for real. [`fetch`] sends a bare `POST` with
  `Authorization: Basic <client_id>:<client_secret>` and no body at all.

**The token response shape itself is not documented anywhere in the
vendored Swagger at all** — `securityDefinitions.OAuth2` names a
`tokenUrl` and a scope and nothing else; there is no `#/definitions/...`
for what `tokenUrl` actually returns. This is a real gap versus Orange's
own copy of this struct: Orange's `expires_in` is confirmed against
`docs/architecture.md` §6.2's own stated figure (3600s), not merely
assumed. MTN has no equivalent confirmation anywhere in this repo.
[`TokenResponse`] assumes the standard `OAuth2` `client_credentials`
response shape (RFC 6749 §5.1: `access_token`, `expires_in`) purely
because it is the shape every `client_credentials` implementation this
crate's authors have seen uses, and because there is nothing else to go
on. This is **unverified**, the same as everything else this crate has
never run against a live MTN account — see `lib.md`'s own honesty
ledger.
