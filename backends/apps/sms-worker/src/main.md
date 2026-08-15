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
