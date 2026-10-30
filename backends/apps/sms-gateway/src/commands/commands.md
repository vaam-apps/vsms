One file per `sms-gateway` subcommand — cleanup PR D, a pure move out of
what used to be a single ~2400-line `main.rs`. `main.rs` still owns the
`Cli`/`Command` clap definitions (the enum variant's own doc comment is
what clap turns into each subcommand's `--help` "about" text, and that
only works if the doc comment stays attached to the variant, so it wasn't
moved) and the dispatch `match`; every variant wraps a `<Name>Args` struct
that lives in this module's own submodule, next to the handler function
that consumes it. That split — enum in `main.rs`, `Args` + handler
together in `commands::<name>` — is what gets the ~150-plus lines of
per-field doc comments and `#[arg(...)]` attributes for each subcommand
out of `main.rs` and into the file that owns the corresponding logic,
without touching where clap actually reads a subcommand's own "why does
this exist" prose from.

There is no `common.rs`: the one helper more than one subcommand needs —
the `system`-role context an internal write runs under — is
`sms_api::system_context`, shared with the worker rather than duplicated
here. Everything used by exactly one subcommand stays private to
that subcommand's own file — `seed_dispatch::seed_dispatch_core` and
`provision_user::create_console_user_if_absent` are the two exceptions,
`pub(crate)` because `bootstrap::bootstrap_command` reuses them directly
rather than re-deriving their logic (see `bootstrap.rs`'s own doc for why).

# Which subcommands read the YAML configuration (VSC-1)

Four take `--config`/`--profile` (`SMS_CONFIG`/`SMS_PROFILE`): `serve`, whose
pepper, pool size, rate limits and providers are in `config/application*.yml`;
`mtn-subscribe-dlr`, whose credentials are the `providers[code=mtn_cm]` entry of
the same document; and `provision-client` and `rotate-signing-key` (VSA-3a), which
read it only to learn whether the deployment serves `machine_tokens` and refuse
before the database when it does not (`Binary::Operator`, no `security:`).
`main.rs` pins the four names. Every other subcommand is a one-shot operator action
on the database and reads no deployment configuration; their arguments stay
arguments. `main.rs` refuses a retired flag (`sms_config::RETIRED`) before clap
parses.

`serve` also reads `auth.methods` (VSA-1): it passes the `hmac_webhooks` answer to
`sms_api::router`, mounts `GET /.well-known/vsms-auth` beside `/healthz` (outside the
envelope, `auth_discovery.md`) and logs the permitted and served methods and the
`hmac` endpoint count at boot.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
