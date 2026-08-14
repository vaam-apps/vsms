# Runbooks

Step-by-step operational procedures — as opposed to [`docs/architecture.md`](../architecture.md), which is the design spec, or [`CONTRIBUTING.md`](../../CONTRIBUTING.md), which is the engineering rules. A runbook is what you actually run, in order, to get a specific outcome.

If you are integrating an application rather than running or operating vsms, [`../integrating.md`](../integrating.md) is the guide you want instead.

| Runbook | For |
|---|---|
| [Local development](local-development.md) | Daily development: `just demo` brings the whole stack up against a fake carrier, with no Orange or MTN account and no real SMS. Credentials per developer, fault injection, and troubleshooting. |
| [Showcase](showcase.md) | The fastest way to see vsms working with no source build: `compose.demo.yaml` pulls every image from GHCR and brings up a console you can sign into, with a message reaching a terminal state. Not for development — see `compose.demo.yaml`'s own header for how it differs from `just demo` and from [Deployment](deployment.md). |
| [Getting started](getting-started.md) | First-time setup, the long way: clone, build, apply migrations, run both binaries by hand, send a real message through the whole pipeline against a scratch database. Read this when something in `just demo` fails and you need to know which link broke. |
| [Deployment](deployment.md) | Running it for real: the compose stack, the Caddy edge, the one-shot migration job, image builds and the GHCR release path. |
| [Backup and restore](backup-restore.md) | The backup job, and the restore drill that proves a backup is actually restorable. |
| [#36 — real handset delivery gate](36-handset-gate.md) | The milestone 2 acceptance gate: a real SMS to a real Orange handset, and a `kill -9` lease-reclaim proof. Needs real Orange Cameroon credentials and a real phone — not runnable in CI or by an agent. |
| [#160 — the joined integration story](e2e-integration.md) | `scripts/e2e-integration.sh` / `just e2e-integration`: an external client (`examples/rust/sms-send`) sends over real HTTP, and that exact message id is read back through the admin console's own data path and credential, reaching `delivered`. Proves integration readiness, not carrier readiness — #36 above is unaffected. |
| [#70/#71 — alerting](alerting.md) | What each of the five Prometheus alerts in `deploy/prometheus/alerts.yml` means and what to do when it fires, plus how to correlate one message across `sms-gateway` and `sms-worker`'s logs end to end. |
