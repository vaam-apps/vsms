The role-selectable worker binary. §7 of the design doc.

This package is `sms-worker-bin`, not `sms-worker` — that name belongs to
the library crate this binary depends on (`backends/crates/sms-worker`), and
Cargo package names must be unique workspace-wide. The `[[bin]]` override
in `Cargo.toml` is what makes the produced executable `sms-worker`
regardless, matching every `sms-worker --roles ...` example in the design
doc.

# #70/#71: this process now has an HTTP surface, deliberately

[`spawn_heartbeat`]'s own doc used to say this binary has none at all —
"six poll-loop roles, never a listener" — true until now. `main` below
binds a **second, separate** listener (`--metrics-listen`, default
`127.0.0.1:9091`) serving exactly one route, `GET /metrics`
(`sms_api::metrics::router()`, reused rather than duplicated — see that
function's own module doc), independent of `--roles` and of the
heartbeat file mechanism below, which stays exactly as it was: the
container `HEALTHCHECK` still has no shell to run a `curl`-shaped check
with (see `spawn_heartbeat`'s own doc for why), and `/metrics` answers a
different question — process observability, not container liveness —
so it does not replace the heartbeat file, it sits alongside it.
`sms_metrics`'s own module doc covers what each of the four gauges this
process's `/metrics` reports actually measures, and why.

# VSC-1: the configuration is a YAML document, shared with the gateway

`--roles`, `--database-url`, `--metrics-listen`, `--worker-id` and
`--envelope-signing-key-file` are process plumbing and stay flags. The pool size
and every provider's credentials are `config/application*.yml`
(`--config`/`--profile`, `SMS_CONFIG`/`SMS_PROFILE`), the same document
`sms-gateway serve` reads, so the two binaries cannot disagree about a provider.
The provider registry is built from `sms_config::Providers`; an `aws_sms` entry
that only has `events_topic_arn` is the gateway's half and registers nothing
here. The flags that carried those values are refused before clap parses, naming
the YAML key that replaced each (`sms_config::RETIRED`).

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
