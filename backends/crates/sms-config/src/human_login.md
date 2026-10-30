`human_login:`, the block that makes the `human_login` method run (VSA-4a, owner
ruling R5).

```yaml
human_login:
  console_client:
    client_id: sms-console
    redirect_uris:
      - https://admin.vsms.example.org/api/auth/callback
  login_rate_limit:
    per_email:
      burst: 5
      refill_per_second: 0.1
    aggregate:
      burst: 10
      refill_per_second: 1.0
```

It is `29f93c4^`'s flags moved into the document the way VSC-1 moved the others:
`console_client.client_id` was `--client-id` / `SMS_CONSOLE_OIDC_CLIENT_ID` of
`seed-console-client` (default `sms-console`), `redirect_uris` was its
`--redirect-uri`, and there was no login limit at all. There is no secret in it:
the console is a **public** client protected by PKCE, and a person's password is
typed at sign-in and kept as an Argon2id hash in `user_credentials`.

```yaml
human_login:
  # console_client and login_rate_limit as above
  sso:
    issuer: https://auth.example.org/realms/staff
    client_id: vsms-gateway
    client_secret: ${SMS_SSO_CLIENT_SECRET}
    scopes: [openid, email, profile]
    required_group: vsms-staff
    groups_claim: groups
    accepted_amr: [otp]
    accepted_acr: []
    jit_role: support
    jwks_min_refresh_secs: 30
    rate_limit: { burst: 30, refill_per_second: 1.0 }
```

`sso:` (VSA-4c) is the deleted `serve` flags `SMS_SSO_*` back as a block, nested
under `human_login:` because a person signs in to the same console either way:
it cannot exist without the block it sits in, and every rule about
`human_login` (`auth.md`) holds for it by construction. It is optional - absent
means the password is the only way in. See "Rules of `sso`" below.

# Permitted is not served

`human_login` is **served** when `auth.methods` permits it, this block is
present **and** `op:` is present ([`crate::AuthMethods::human_login_served`]): the
sign-in is the provider's `authorization_code` flow, so without `op:` there is
nothing to sign in to. See `auth.md` for the four rules that follow from it. The
block is not defaulted and not derived from `deployment.public_base_url`: the
console's callback is a statement a deployment makes about its own console, and a
redirect URI that was guessed is one the provider refuses for a reason nobody
can see. A `null` value (`human_login:` with every key commented out) is a shape
error naming the missing keys, not an empty block.

# The two buckets, and no third

Both are taken before any password is hashed (`sms-gateway`'s `login.md` has the
order and the reasoning in full).

- **`per_email`**: one bucket per email address, trimmed and lowercased. What one
  account can be tried with. The example (burst 5, one more try every ten seconds)
  is the figure the first draft applied to the source address as well.
- **`aggregate`**: one bucket for every sign-in together, the Argon2 CPU budget.
  The example is burst 10 and one a second: an Argon2id verification at the default
  cost is tens of milliseconds of a core and 19 MiB, so a tenth of the API's own
  per-peer refill (`sms_api::router`'s `RateLimitConfig::new(1200, 10.0)`, which
  admits calls a fraction of that price) keeps the hashing to a few percent of one
  core, and ten bursts to about 190 MiB. Retune it with the cores and the memory the
  `serve` pod has.

**There is no bucket per source address.** Behind the edge proxy and the console's
own server the gateway sees one peer for everybody, so a "per source" bucket is a
global one: at the first draft's burst of 5 and refill of 0.1, one bad request every
ten seconds kept every operator out. A single source is bounded at the edge
instead (`deploy/Caddyfile`, `auth_per_source`), where its address is known.

# Rules

- `console_client.client_id` is not blank, has no whitespace and is 8 to 64
  characters (`OauthClient.clientId`'s own `@length`);
- `console_client.redirect_uris` has at least one entry. Each parses as a URL,
  carries no fragment (RFC 6749, section 3.1.2), no whitespace (the registration
  stores the list space-delimited), is listed once, and under `livemode` is
  `https` with a real host (`ConfigError::InsecureHost`,
  `ConfigError::StubHostInLivemode`, naming the entry by index);
- `login_rate_limit.per_email` and `login_rate_limit.aggregate` are each a bucket
  with a burst of at least 1 and a positive, finite refill, like the `api`
  buckets (`ConfigError::InvalidValue`, naming the bucket's own key). Both are
  required and nothing else is allowed under `login_rate_limit`: a `per_source`
  key is refused as an unknown field, because there is none (below). The flat
  form this block first had (`burst` and `refill_per_second` directly under it) is
  a shape error naming `per_email`.

The list and the id are the **truth `serve` holds the database row to**: it
refuses to start when the `OauthClient` row the provider reads differs
(`sms-gateway seed-console-client` reconciles it), so a changed redirect URI in
the file is never silently ignored by an old row.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->

# The callback is derived

There is no `redirect_uri` key: writing one is an unknown-field error. The
callback registered at the identity provider is `op.issuer` followed by
`/login/sso/callback`, and `serve` logs it at boot, so there is no second
statement of the gateway's own origin to disagree with the first. The old
`SMS_SSO_REDIRECT_URI` had to "end in exactly `/login/sso/callback`" and nothing
else about it could be checked against the `op:` issuer; deriving it removes the
way to get it wrong.

# Rules of `sso`

- `issuer` is a URL with a host and no query, fragment or credentials. It is
  `https` in every mode, except plain `http` to a loopback host (a developer's
  Keycloak, the test suite's fake), because the client secret and the authorization
  code travel over it. Under `livemode` it also passes the real-host rule of every
  other URL (`https`, no stub host). It is kept **verbatim**: the discovery
  document's `issuer` and every `id_token`'s `iss` are compared to it byte for
  byte, so no trailing slash is added or trimmed.
- `client_id` is not blank and has no whitespace.
- `client_secret` is a secret like the pepper: under `livemode` a literal is
  `ConfigError::LiteralSecret`, any mode refuses a YAML number or boolean and a
  blank value, and `Debug` prints `<redacted>`. **Only the gateway is handed it**:
  the loader drops `human_login.sso` for the worker and the one-shot commands
  after the retired-variable and literal checks and before placeholders resolve,
  so a worker starts without `SMS_SSO_CLIENT_SECRET` in its environment
  ([`crate::Config::sso`] is `None` there).
- `scopes` is non-empty, each entry one scope with no whitespace, none repeated,
  and `openid` among them.
- `required_group` and `groups_claim` are not blank. The group is an exact match
  against the claim, and it is a **gate**: no claim ever sets a role.
- `accepted_amr` and `accepted_acr` are each a list of trimmed, non-blank,
  distinct values (either may be `[]`), and **not both empty**: an empty rule
  would read as no rule.
- `jit_role` is a `Role.key` (3 to 32 of `a-z0-9_`, starting with a letter) and not
  one of [`REFUSED_JIT_ROLES`]: `system` and `app` (reserved, no human holds them)
  and `owner` and `admin` (they act only from a paired console key, so an account
  created by an unsupervised sign-in must not carry either). The role row's
  existence is checked at boot by `serve`, which this crate cannot see.
- `jwks_min_refresh_secs` is 1 to 3600.
- `rate_limit` is a bucket like the others. It is one **global** bucket per
  route (`/login/sso/start` and `/login/sso/callback`), not one per source: the
  gateway sees one peer for everybody, as `login_rate_limit` explains. The edge
  bounds a single source (`deploy/Caddyfile`, `sso_per_source`).
