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
the `system`-role context every OP-adjacent write runs under — is
`sms_api::system_context`, shared with the worker rather than duplicated
here. Everything used by exactly one subcommand stays private to
that subcommand's own file — `seed_dispatch::seed_dispatch_core` and
`seed_console_client::seed_console_client_core` are the two exceptions,
`pub(crate)` because `bootstrap::bootstrap_command` reuses them directly
rather than re-deriving their logic (see `bootstrap.rs`'s own doc for why).
