The loaded document and the loader.

The steps, in order, and why they are in this order:

1. The base file must exist. figment alone treats a missing file as an empty
   document, which would turn a typo into errors about keys the operator never
   wrote.
2. figment merges the base and `application-<profile>.yml` if it exists. A
   profile that names no file loads the base alone, as vpay's does.
3. **Retired variables are refused, on the text as written.** A retired variable
   that the document reads as `${NAME}` is a secret's source and is fine.
4. **Literal secrets are listed, on the text as written.** After step 5 a
   literal and a resolved placeholder are the same string. The list is only
   acted on under `deployment.livemode`.
5. `${VAR}` placeholders resolve; one the environment lacks is fatal.
6. The document is deserialised with every block refusing unknown keys.
7. The cross-field rules run: livemode's `https` and stub-host rules, the
   numeric ranges, each provider's required keys.

`Config::load_with_env` takes the environment as a closure because
`std::env::set_var` is `unsafe` in edition 2024 and this workspace forbids
`unsafe`.

```
use sms_config::{Config, ConfigError};

let typo = std::path::Path::new("config/aplication.yml");
let error = Config::load(Some(typo), "dev").unwrap_err();
assert!(matches!(error, ConfigError::FileNotFound(ref named) if named.contains("aplication")));
```

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
