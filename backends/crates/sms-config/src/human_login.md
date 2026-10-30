`human_login:`, the block that makes the `human_login` method run (VSA-4a, owner
ruling R5).

```yaml
human_login:
  console_client:
    client_id: sms-console
    redirect_uris:
      - https://admin.vsms.example.org/api/auth/callback
  login_rate_limit:
    burst: 5
    refill_per_second: 0.1
```

It is `29f93c4^`'s flags moved into the document the way VSC-1 moved the others:
`console_client.client_id` was `--client-id` / `SMS_CONSOLE_OIDC_CLIENT_ID` of
`seed-console-client` (default `sms-console`), `redirect_uris` was its
`--redirect-uri`, and there was no login limit at all. There is no secret in it:
the console is a **public** client protected by PKCE, and a person's password is
typed at sign-in and kept as an Argon2id hash in `user_credentials`.

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

# Rules

- `console_client.client_id` is not blank, has no whitespace and is 8 to 64
  characters (`OauthClient.clientId`'s own `@length`);
- `console_client.redirect_uris` has at least one entry. Each parses as a URL,
  carries no fragment (RFC 6749, section 3.1.2), no whitespace (the registration
  stores the list space-delimited), is listed once, and under `livemode` is
  `https` with a real host (`ConfigError::InsecureHost`,
  `ConfigError::StubHostInLivemode`, naming the entry by index);
- `login_rate_limit` is a bucket with a burst of at least 1 and a positive
  refill, like the `api` buckets (`ConfigError::InvalidValue`).

The list and the id are the **truth `serve` holds the database row to**: it
refuses to start when the `OauthClient` row the provider reads differs
(`sms-gateway seed-console-client` reconciles it), so a changed redirect URI in
the file is never silently ignored by an old row.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
