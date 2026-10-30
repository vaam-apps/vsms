Verifying the access tokens this gateway's own OP mints, so the API can accept
one (VSA-3b for a machine, VSA-4b for a person).

[`OpBearerVerifier`] implements `sms_api::BearerVerifier`. It answers one
question: **did this process's OP sign this token, and is it an access token of a
realm this deployment serves, for this issuer, that has not expired.** What a
verified token may then do is `sms_api`'s to decide, from rows
(`docs/architecture.md` section 4.9).

# The realms

[`Realms`] says whose tokens are accepted, and is built from what `auth.methods`
**serves** (`commands::serve::realms`), with no key of its own: the machine realm
is `machine_tokens` served, the person realm is `human_login` served, addressed to
`human_login.console_client.client_id`. The realm of a token is its own
`identity` claim: a person's token carries one and a `client_credentials` token
never does. A token of a realm not served is refused for that, as
`HumanToken` or `MachineToken`, after the signature, issuer, expiry and `cnf`
have been checked and before anything else about it is read.

# What it checks

| Check                    | Refusal             | How                                                                         |
| ------------------------ | ------------------- | --------------------------------------------------------------------------- |
| three segments, RS256    | `Malformed`         | `decode_header`; any other algorithm is refused before a key is looked up   |
| a `kid`                  | `MissingKid`        | never the first key of the set: during a rotation two keys are published    |
| a key for that `kid`     | `UnknownKid`        | the OP's own keyring, then one forced reload (below)                        |
| the signature            | `Signature`         | RSA, under that key                                                         |
| `iss` is `op.issuer`     | `Issuer`            | required, exact                                                             |
| `exp`, `nbf`             | `Expired`           | 60 seconds of leeway; `exp` required; `nbf` is `NotYetValid`                |
| no `cnf`                 | `SenderConstrained` | a token bound to a key needs that key's proof, which this door never checks |
| `identity`, a person     | `HumanToken`        | when no person realm is served                                              |
| no `identity`, a machine | `MachineToken`      | when no machine realm is served                                             |

A **machine** token (no `identity`) then needs `aud` to contain `sub`
(`Audience`, by hand after the signature: authkestra stamps `aud = client_id`).
A **person** token (an `identity`) then needs:

| Check                                            | Refusal     | How                                                                                                      |
| ------------------------------------------------ | ----------- | -------------------------------------------------------------------------------------------------------- |
| `sub` non-empty and the identity's `external_id` | `Malformed` | the provider stamps `sub` from `identity.external_id`; a token where they differ is not its              |
| exactly one `aud`, the console                   | `Audience`  | a string, or a list of one, equal to the console's `client_id`; any other list is refused                |
| a non-blank `scope`, no `nonce`                  | `IdToken`   | the ID token has no scope (and a nonce when the sign-in sent one); the access token has `openid profile` |

The machine audience rule is "contains `sub`", not "equals `vsms`": a token is addressed
to its own client by default, which is what keeps the SDKs that already speak to
the provider working, and a client that asked for another audience gets a token
for that resource and not for this API. `validate_aud` is off in the library
because its audience is one fixed list and this one differs per token; the hand
check replaces it and a test fails when it is removed.

Nothing is taken from the token except who it names (a machine's client id and
the scope it **asks for**; a person's `User.id` and the console it was minted
for). Role, permissions, app and the scopes it holds all come from rows.

The person audience is stricter than the machine's on purpose. A machine's
`aud` is its own `sub`, so "contains" is the only rule that fits; a person's is
the console, one fixed value, and a token that lists another resource beside it
is a token for two audiences that this API does not serve.

# The ID token is the one confusion that needs a heuristic

The provider's ID token (`TokenManager::issue_id_token`) carries the same `iss`,
`sub`, `aud`, `identity`, `kid` and lifetime as the access token beside it, and
differs only in `scope` (none) and an optional `nonce`. The console reads it in a
browser to learn who signed in and may log it; if it opened the API, leaking one
would hand out a session. `authkestra` stamps no `typ` or `token_use`, so the
distinction is the absence of a scope or the presence of a nonce: `IdToken`.
The cost is that a sign-in that asked for no scope gets an access token with no
scope and the API refuses it; `human_login`'s console client is registered with
`openid profile` and `/login` is asked for it. An upstream request for a
distinguishing claim is the proper fix.

# The keys are the OP's, in process

`/jwks.json` and this verifier read one keyring (`crate::op::OpKeyring`), under
one lock: the key set `/jwks.json` publishes and the keys tokens are verified
with are the same value, published at the same instant, key set before signing
key (`Keyring::publish`), so a token can never carry a `kid` the verifier has not
got yet. There is **no HTTP fetch of our own JWKS** and no cache of the verifier's
own: a door that needs the network to recognise the gateway's tokens breaks when
the gateway is slow, and one that can be pointed at another JWKS URL accepts that
other party's tokens.

# An unknown `kid` forces one reload

`rotate-signing-key` run against a running gateway reaches it through
`spawn_key_refresh`, every `op.signing_key.refresh_interval_secs`. A second
gateway replica, or a token minted by a replica that already refreshed, can name
a `kid` this process has not loaded yet. Rather than refuse every such token for
up to a refresh interval, the verifier reloads the key set once.

```mermaid
sequenceDiagram
    participant C as Caller
    participant V as OpBearerVerifier
    participant K as OpKeyring
    participant DB as OauthSigningKey rows
    C->>V: token naming kid N
    V->>K: key for N?
    K-->>V: none
    V->>V: lock; N loaded meanwhile? last reload under 5 s ago?
    alt allowed
        V->>DB: load_signing_keys (2 s deadline)
        DB-->>V: tokens + JWKS, or nothing in time
        V->>K: refresh (key set first)
    end
    V->>K: key for N?
    K-->>V: key, or none: UnknownKid
```

The reload is **single flight and rate limited** ([`MIN_FORCED_RELOAD`], five
seconds): the lock is held across the read, so concurrent unknown `kid`s wait for
the one in flight, and a flood of invented `kid`s costs one database read per
interval and not one each.

It also has a **deadline** ([`RELOAD_TIMEOUT`], two seconds). The read runs
under that lock, so a database that stalls instead of failing would otherwise
park every request naming an unknown `kid` for as long as the stall lasts. On
the deadline the read is dropped, the lock is released, a `warn` is logged (the
deadline only, never the token) and the caller is refused as `UnknownKid`,
which is the same coarse 401 as any other refusal. The attempt still counts as
the interval's reload.

That bounds the cost of a stall to one deadline per interval, and it has a
price worth knowing: **an attacker who triggers a reload can leave a key that
another replica just rotated in refused for up to five seconds.** The invented
`kid` spends the interval's one reload (whether it succeeded, failed or timed
out), and a real token naming the new key within that window finds the verifier
unwilling to read again. It self-heals: the next unknown `kid` after the window
reloads, and the periodic refresh (`op.signing_key.refresh_interval_secs`)
reloads regardless. Nothing is accepted that should not be; a token is refused
a little longer than it had to be, and the caller asks again.

# What is deliberately not here

- `authkestra_resource`'s `JwksCache`: it fetches the JWKS over HTTP, falls
  back to the first key without a `kid`, and caches for a fixed time.
- `validate_aud = false` with nothing in its place.
- `scope` or `perms` read from the token as a grant.
- A person's `scope` or `identity` attributes read as a grant, or a role
  taken from the token. The role is the `User` row's.
- `provider_id` is not checked: the cap and the standing are keyed on the
  credential and the account, so a later sign-in through another provider that
  maps onto a `User.id` is held to the same rules.
- Sender-constrained tokens. The provider mints none today, but a `cnf` claim
  (RFC 7800) means the token is only good with a proof of possession, and a
  door that checks none must not wave it through as a bearer: it is
  `SenderConstrained`, whatever the claim holds.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
