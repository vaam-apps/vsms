# Runbooks

Step-by-step operational procedures — as opposed to [`docs/architecture.md`](../architecture.md), which is the design spec, or [`CONTRIBUTING.md`](../../CONTRIBUTING.md), which is the engineering rules. A runbook is what you actually run, in order, to get a specific outcome.

| Runbook | For |
|---|---|
| [Getting started](getting-started.md) | First-time setup: clone, build, apply migrations, run both binaries locally, send a real message through the whole pipeline against a scratch database. |
| [#36 — real handset delivery gate](36-handset-gate.md) | The milestone 2 acceptance gate: a real SMS to a real Orange handset, and a `kill -9` lease-reclaim proof. Needs real Orange Cameroon credentials and a real phone — not runnable in CI or by an agent. |
| [#160 — the joined integration story](e2e-integration.md) | `scripts/e2e-integration.sh` / `just e2e-integration`: an external client (`examples/rust/sms-send`) sends over real HTTP, and that exact message id is read back through the admin console's own data path and credential, reaching `delivered`. Proves integration readiness, not carrier readiness — #36 above is unaffected. |
| [#70/#71 — alerting](alerting.md) | What each of the five Prometheus alerts in `deploy/prometheus/alerts.yml` means and what to do when it fires, plus how to correlate one message across `sms-gateway` and `sms-worker`'s logs end to end. |
