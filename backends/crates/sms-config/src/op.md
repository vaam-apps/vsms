`op:`, the block that makes the `machine_tokens` method run (VSA-3a, owner ruling
R5).

```yaml
op:
  issuer: https://vsms.example.org
  token_rate_limit:
    burst: 10
    refill_per_second: 0.3333333333333333
  signing_key:
    refresh_interval_secs: 60
    rotation_overlap_secs: 1800
```

It is `29f93c4^`'s flags moved into the document the way VSC-1 moved the others:
`issuer` was `--issuer` / `SMS_OIDC_ISSUER`, `token_rate_limit` was
`--token-rate-limit-burst` / `--token-rate-limit-refill-per-second`, and the two
`signing_key` values were constants of the provider
(`DEFAULT_KEY_REFRESH_INTERVAL`, `ROTATION_OVERLAP`) that a deployment could not
change. There is no secret in it: the signing keys are database rows
(`oauth_signing_keys`), minted by `sms-gateway rotate-signing-key`, never written
in a file.

# Permitted is not served

The block is the second half of a two-part condition. `machine_tokens` is
**served** when `auth.methods` permits it **and** `op:` is present
(`AuthMethods::machine_tokens_served`); see `auth.md` for the three rules that
follow from it. The block is not defaulted and not derived from
`deployment.public_base_url`: an issuer is a statement a deployment makes about
itself, and a token whose `iss` was guessed would be one a verifier rejects for a
reason nobody can see. A `null` value (`op:` with every key commented out) is a
shape error naming the missing keys, not an empty block.

# Rules

- `op.issuer` parses as a URL, has no trailing slash, and under `livemode` is
  `https` with a real host (`ConfigError::InsecureHost`,
  `ConfigError::StubHostInLivemode`);
- `op.token_rate_limit` is a bucket with a burst of at least 1 and a positive
  refill, like the `api` buckets;
- `op.signing_key.refresh_interval_secs` is at least 1;
- `op.signing_key.rotation_overlap_secs` exceeds [`ACCESS_TOKEN_TTL_SECS`]
  (`ConfigError::InvalidValue` each).

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
