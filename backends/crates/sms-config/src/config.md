The loaded document and the loader.

The steps, in order, and why they are in this order:

1. The base file must exist. figment alone treats a missing file as an empty
   document, which would turn a typo into errors about keys the operator never
   wrote.
2. figment merges the base and `application-<profile>.yml`. **A profile that
   names no file is an error naming the file it expected**
   ([`ConfigError::UnknownProfile`]), so a typo (`prodd`) cannot boot on the base
   alone and look like a choice. The default profile is exempt: a process that
   names nothing may run on the base file alone.
3. **Retired variables are refused, on the text as written.** A retired variable
   that the document reads as `${NAME}` is a secret's source and is fine.
4. **Literal secrets are listed, on the text as written.** After step 6 a
   literal and a resolved placeholder are the same string. The list is only
   acted on under `deployment.livemode`. A secret YAML reads as a number or a
   boolean (`client_secret: 123456`) is refused here by key, because serde's own
   error for it would print the value.
5. **Only the gateway keeps `security`.** [`Binary::Worker`] and
   [`Binary::ProviderTool`] (`sms-gateway mtn-subscribe-dlr`, which calls a
   provider and hashes nothing) remove the block after steps 3 and 4 have seen
   it and before step 6 resolves it, so neither requires nor receives
   `SMS_HASH_PEPPER`. The gateway ([`Binary::Gateway`]) keeps the block and
   refuses to start without the pepper.
6. `${VAR}` placeholders resolve; one the environment lacks is fatal. The
   secret keys are known by now (step 4), so a failure is reported according to
   where it happened. Under a secret key, a value that is not exactly one
   `${VAR}` and fails to resolve (an unclosed `${`, a bad name, or a placeholder
   the environment does not set, with text around it, as in `${SET}${UNSET}`) is
   [`ConfigError::SecretNotAPlaceholder`]: the key path and nothing else, no
   fragment and no variable name, because the string is a literal secret or has
   one around its placeholder, and resolve runs before the livemode refusal of a
   literal. **So is a `${NAME}` whose `NAME` is not shaped like an environment
   variable, `^[A-Z_][A-Z0-9_]*$`, whole or not, and whatever the environment
   holds under that name:** `client_secret: "${s3cr3tAlnumValue9}"` is a secret
   pasted over the variable's name, valid to the resolver and not a placeholder
   anybody set, so it counts as a literal (refused in a livemode document as one,
   and by this error before that) and its name is never printed. Every
   placeholder in `config/*.yml` has the shape. A secret written as an array or a
   map (`client_secret: ["..."]`) is hidden at every depth: `...client_secret[0]`
   and `...client_secret.x` are inside the secret and get the same error. Only a
   whole, well-shaped `${VAR}` that is unset still names `VAR`, which is a
   variable name and not the secret. Anywhere else a malformed placeholder is
   reported by YAML key and the offending `${...}` fragment, capped. Both name a
   provider the one way, `providers[code=orange_cm]`.
7. The document is deserialised with every block refusing unknown keys.
8. The cross-field rules run: livemode's `https` and stub-host rules, the
   numeric ranges, each provider's required keys.

What an error message may repeat is a rule of this crate: a URL, a key, a
variable name, a capped placeholder fragment. Never a secret's text. serde's
`Shape` errors echo the value they could not read, so the values that can be
secret are checked before serde sees them (step 4), and an unknown key or a
wrong type elsewhere in the document echoes only what is not secret.

`Config::load_with_env` takes the environment as a closure because
`std::env::set_var` is `unsafe` in edition 2024 and this workspace forbids
`unsafe`.

```rust
use sms_config::{Binary, Config, ConfigError};

let typo = std::path::Path::new("config/aplication.yml");
let error = Config::load(Some(typo), "dev", Binary::Gateway).unwrap_err();
assert!(matches!(error, ConfigError::FileNotFound(ref named) if named.contains("aplication")));
```

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
