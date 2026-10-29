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

| Method           | What it permits                                                                              | Where it is enforced                                                                                                 |
| ---------------- | -------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `keypair`        | requests signed by a registered key, and the sealed answers; the console's paired keys       | the envelope layer, always `Required`                                                                                |
| `machine_tokens` | `Authorization: Bearer` from the deployment's own `OpenID` Provider                          | **not restored** (VSA-3)                                                                                             |
| `hmac_webhooks`  | webhook endpoints delivered with `X-Sms-Signature` (`deliveryMode: hmac`), and their secrets | `sms_api::HmacWebhooks`: the create, update, switch and rotate calls refuse; `sms-worker`'s `hooks` parks deliveries |
| `human_login`    | a person's password or SSO sign-in to the console                                            | **not restored** (VSA-4)                                                                                             |

# Permitted is not served

A method on the list is *permitted*. [`AuthMethods::served`] is what a client
may be told, and what `GET /.well-known/vsms-auth` answers: the permitted methods
this release has code for. `machine_tokens` and `human_login` are permitted by
the default and served by nothing, so the default lists two of the four; naming
either explicitly is refused ([`ConfigError::AuthMethodUnavailable`]) rather than
promising a door that is not there. Restoring one (VSA-3, VSA-4) changes
`AuthMethod::is_restored` and nothing else here.

# Rules

One [`ConfigError`] variant each:

1. an empty list or a null, [`ConfigError::NoAuthMethods`];
2. an unknown name, [`ConfigError::UnknownAuthMethod`];
3. a duplicate, [`ConfigError::DuplicateAuthMethod`];
4. `machine_tokens` or `human_login` named explicitly,
   [`ConfigError::AuthMethodUnavailable`];
5. nothing permitted can authenticate a caller of the API,
   [`ConfigError::NoWayIn`]. Today only `keypair` can, so `[hmac_webhooks]` alone
   is refused: a gateway that would answer every request with the unsigned 401.

vpay has a sixth, a block present for a method that is not permitted
(`AuthMethodDisabled`). vsms has no such block yet: `keypair` has no YAML (keys
are database rows), `hmac_webhooks` has none either (endpoints are database rows
too, so the gate is at run time, not at boot), and the blocks `machine_tokens`
and `human_login` will bring (`op:`, `human_login:`) do not exist, so the loader
already refuses them as unknown keys. The rule arrives with the first of them.

# Examples

```rust
use sms_config::{AuthConfig, AuthMethod};

let absent: AuthConfig = serde_yaml_ng::from_str("{}").expect("an absent key");
let all = absent.resolve().expect("absent is always legal");
assert!(all.permits(AuthMethod::Keypair) && all.permits(AuthMethod::HmacWebhooks));
assert!(all.permits(AuthMethod::MachineTokens), "permitted ...");
assert!(
    !all.served().any(|method| method == AuthMethod::MachineTokens),
    "... and not served"
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
