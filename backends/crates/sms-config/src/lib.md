YAML deployment configuration for `sms-gateway` and `sms-worker` (VSC-1).

Deployment behaviour lives in `config/application.yml` and a profile overlay
(`config/application-<profile>.yml`); process plumbing stays on the command line
and in the environment. The split is vpay's, and the loader is modelled on
`vpay-config`:

| Stays a flag or a variable | Moves to the YAML |
| -------------------------- | ----------------- |
| `DATABASE_URL`, `--listen`, `--metrics-listen` | pool sizes, `database.*` |
| `SMS_ENVELOPE_SIGNING_KEY_FILE` and `..._NEXT_...` (paths) | the hash pepper, `security.*` |
| `SMS_WORKER_ROLES`, `SMS_WORKER_ID`, `SMS_WORKER_HEALTH_FILE` | idempotency and rate limits, `api.*` |
| `RUST_LOG` | every provider's host, settings and credentials, `providers` |
| `SMS_CONFIG`, `SMS_PROFILE` | `deployment.*` |

# Loading

[`Config::load`] merges the base file with its profile overlay (figment: maps
merge key by key, a list in the overlay replaces the base's list wholesale),
refuses a retired variable, resolves `${VAR}` placeholders against the
environment, round-trips the result through `serde_yaml_ng` into the typed
model, and validates it. A `${VAR}` the environment does not set is an error,
never an empty string. There is no implicit path:

```
use sms_config::{Config, ConfigError};

assert_eq!(Config::load(None, "dev").unwrap_err(), ConfigError::MissingPath);
```

# The hard cutover

The flags and variables that used to configure these binaries are gone, and
they are refused rather than ignored: see [`retired`]. clap silently ignores an
environment variable it no longer declares, so without that refusal a
deployment that still set `ORANGE_CM_CLIENT_ID` would boot with Orange missing.

# Room for `auth.methods`

Every block refuses unknown keys, so an `auth:` key is refused today. VSA-1
adds the block as one more field of the document, between `deployment` and
`security`, and flips that refusal into its own rules.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
