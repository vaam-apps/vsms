`auth.methods`: which ways of authenticating a deployment permits (VSA-1, owner
ruling R5).

vsms serves more than one application, and what each of them authenticates with
is a property of the deployment, not of the binary. The YAML key is

```yaml
auth:
  methods: [keypair, machine_tokens, hmac_webhooks, human_login]
```

and **absent means all four**: a configuration written before the key existed
gets exactly what it always got. Order is irrelevant, a duplicate is refused,
and so are an empty list, a null (`methods:` with every item commented out reads
as one) and an unknown name. Absent and null are different things on purpose:
the first is an operator who never narrowed the list, the second one who tried to
and left it empty, and reading the second as the first would open every door the
edit was meant to shut. vaam writes `methods: [keypair]`.

This is vpay's key (`vpay-config`, VPA-1), with the same rules and the same
spellings, so one operator's muscle memory covers both repositories.

| Method           | What it permits                                                                              | Where it is enforced                                                                                                                                                   |
| ---------------- | -------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `keypair`        | requests signed by a registered key, and the sealed answers; the console's paired keys       | the envelope layer, always `Required`                                                                                                                                  |
| `machine_tokens` | `Authorization: Bearer` from the deployment's own `OpenID` Provider                          | the provider (`/token`, `/jwks.json`, discovery) is mounted when `op:` is present (VSA-3a); the API accepts its tokens beside the envelope (VSA-3b, `sms_api::bearer`) |
| `hmac_webhooks`  | webhook endpoints delivered with `X-Sms-Signature` (`deliveryMode: hmac`), and their secrets | `sms_api::HmacWebhooks`: the create, update, switch, rotate and replay calls refuse; `sms-worker`'s `hooks` claims no `hmac` attempt, which stays `pending`            |
| `human_login`    | a person's password or SSO sign-in to the console                                            | **not restored** (VSA-4)                                                                                                                                               |

# Permitted is not served

A method on the list is *permitted*. [`AuthMethods::served`] is what a client
may be told, and what `GET /.well-known/vsms-auth` answers: the permitted methods
this release has code for **and whose block is present**. `human_login` is
permitted by the default and served by nothing (VSA-4); naming it explicitly is
refused ([`ConfigError::AuthMethodUnavailable`]) rather than promising a door
that is not there. `machine_tokens` is back (VSA-3a) and follows vpay's rule for
a method with a block: it runs when it is permitted **and** `op:` is present
([`AuthMethods::machine_tokens_served`], the one question every part of the
gateway asks, so none can disagree). A deployment that never wrote `op:` keeps
booting under the absent default and simply serves no tokens.

# Rules

One [`ConfigError`] variant each:

1. an empty list or a null, [`ConfigError::NoAuthMethods`];
2. an unknown name, [`ConfigError::UnknownAuthMethod`];
3. a duplicate, [`ConfigError::DuplicateAuthMethod`];
4. `human_login` named explicitly, [`ConfigError::AuthMethodUnavailable`];
5. nothing permitted can authenticate a caller of the API,
   [`ConfigError::NoWayIn`]: vpay's rule is `!keypair && !machine_tokens` (R5
   holds the two repositories to one rule) and `human_login` is not a way in for
   an API caller. **Since VSA-3b the API accepts a machine token, so
   `machine_tokens` counts:** `[machine_tokens]` alone, with its `op:` block, is
   a legal list and boots a gateway whose only way in is a bearer token (the
   signed envelope then refuses a `COSE_Sign1` body with a 415, and pairing a
   console is unavailable until VSA-5). `[hmac_webhooks]` alone is still
   refused. `[keypair, machine_tokens]` boots and serves `/token`.
6. `machine_tokens` named explicitly with no `op:` block,
   [`ConfigError::AuthMethodNotConfigured`] (the absent default never raises it);
7. an `op:` block on a list that does not permit `machine_tokens`,
   [`ConfigError::AuthMethodDisabled`]: dormant configuration is how an operator
   comes to believe a door is shut that is open.

vpay has the same two for `human_login` (`staff_auth` and `dashboard_client`);
vsms's `human_login:` block arrives with VSA-4. `keypair` has no YAML (keys are
database rows) and `hmac_webhooks` has none either (endpoints are database rows
too, so the gate is at run time, not at boot).

# Examples

```rust
use sms_config::{AuthConfig, AuthMethod};

let absent: AuthConfig = serde_yaml_ng::from_str("{}").expect("an absent key");
let all = absent.resolve().expect("absent is always legal");
assert!(all.permits(AuthMethod::Keypair) && all.permits(AuthMethod::HmacWebhooks));
assert!(all.permits(AuthMethod::MachineTokens), "permitted ...");
assert!(
    !all.served().any(|method| method == AuthMethod::MachineTokens),
    "... and not served without an `op:` block"
);

let keypair_only: AuthConfig =
    serde_yaml_ng::from_str("methods: [keypair]").expect("the vaam shape");
let only = keypair_only.resolve().expect("a legal list");
assert!(only.permits(AuthMethod::Keypair));
assert!(!only.permits(AuthMethod::HmacWebhooks));

// `methods:` with every item commented out is a null: not the default.
let null: AuthConfig = serde_yaml_ng::from_str("methods: ~").expect("a null parses");
assert!(null.resolve().is_err());
```

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
