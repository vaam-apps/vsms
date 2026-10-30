Flags and environment variables VSC-1 retired (and, for the eleven `SMS_SSO_*`,
VSA-4c), and the refusal that names the
YAML key which replaced each.

[`RETIRED`] is the single list. It is read by:

- [`reject_retired_flags`] (this module's `check_args`), which both binaries
  call on `std::env::args_os()` before clap parses, because afterwards clap's own
  answer is "unexpected argument" and says nothing of where the value went;
- the loader, which refuses a retired variable that is set unless the YAML reads
  it as `${NAME}`;
- each binary's test that its clap definitions no longer declare any entry.

```rust
use sms_config::{ConfigError, reject_retired_flags};

let error = reject_retired_flags(["serve", "--hash-pepper", "x"]).unwrap_err();
assert_eq!(
    error,
    ConfigError::RetiredFlag {
        flag: "--hash-pepper".to_owned(),
        key: "security.hash_pepper".to_owned(),
    }
);
```

`SMS_SSO_CLIENT_SECRET` is retired as a flag and kept as a secret's source, the
way the pepper is: `human_login.sso.client_secret: ${SMS_SSO_CLIENT_SECRET}` makes
it fine, and a gateway that sets it without referencing it is refused. So is one
that sets any of the other ten, which is the cutover VSA-4c is breaking on
purpose. `SMS_SSO_REDIRECT_URI` points at `op.issuer`, because the callback is
derived from it and no longer written.

A variable that is set to the empty string counts as set: Docker Compose passes
`FOO=` through, and an old `.env` full of blanks is exactly the leftover this
exists to catch.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
