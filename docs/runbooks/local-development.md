# Runbook: local development without a carrier

How to run the whole system on a laptop — gateway, worker, admin console, and a fake Orange — with no Orange or MTN account and no SMS ever reaching a real handset.

This is the runbook for daily development, whether you are working on vsms or against it. [`getting-started.md`](getting-started.md) is the longer, first-time walkthrough that builds the same thing by hand, one step at a time, and explains what each step proves; read that one when something here fails and you need to know which link broke. If you are integrating an application, [`../integrating.md`](../integrating.md) is what you want next.

## The one command

```bash
just demo
```

Everything runs as a container, built from this checkout's own source — `compose.dev.yaml` (see that file's own header for the full design, including why it's a third, separate compose file alongside `compose.yml` and `compose.demo.yaml` rather than a reuse of either). From a cold start this takes a few minutes (it builds every image). It brings up, in order: a scratch Postgres, both migrations, an OP signing key, an `App` + `Provider` + `Route` + `SenderId`, a machine client, the `sms-console` OIDC client, a human operator account, `sms-fake-orange`, `sms-gateway`, `sms-worker` (`dispatch,scheduler,jobs`), and the admin console.

```bash
just demo-status   # what's running
just demo-login    # the generated demo@vsms.local password — printed once, to a container log
just demo-down     # stop everything, remove its own volumes only
```

Default ports (override via env if these collide with something already running on your machine — every one of the seven services in `compose.dev.yaml`'s own build already races other parallel work on a shared Docker daemon in some environments, so collisions are real, not hypothetical):

| Var | Default | What |
|---|---|---|
| `VSMS_DEMO_PG_PORT` | `15433` | Postgres, published on loopback |
| `VSMS_DEMO_GATEWAY_PORT` | `8080` | `sms-gateway` |
| `VSMS_DEMO_ORANGE_PORT` | `8090` | `sms-fake-orange` |
| `VSMS_DEMO_CONSOLE_PORT` | `3100` | the admin console |

```bash
VSMS_DEMO_GATEWAY_PORT=18080 just demo
```

`just demo` resets to a genuinely fresh state on every `up` — `compose.dev.yaml`'s own named volumes are wiped first (`down -v`), not a targeted `DROP DATABASE`/`CREATE DATABASE` the way the pre-containerisation version of this script used to do it. It is meant to be re-run, not kept alive for days.

## Why there is no carrier

`sms-fake-orange` ([`app/sms-fake-orange`](../../app/sms-fake-orange)) impersonates Orange Cameroon's token and submit endpoints, and independently POSTs delivery receipts back to the gateway's `POST /dlr/{providerKey}` route. It is a **participant, not a response stub** — "the SMS never arrived" is the absence of a later callback, which a mock that only answers the submit call can never model.

Nothing in the gateway or the worker knows it exists. The only difference from production is `ORANGE_CM_BASE_URL`. Every other code path — routing, submission, DLR ingestion, the state machine, webhooks — is the one that runs in production.

**Never point a deployment at it.** It logs a `WARN` saying so on every start, and no production compose file (`deploy/docker-compose.yml`) references it.

## Where everything actually is

There is no `.demo/` directory of pidfiles and logs any more — every process is a container, so `docker` already owns process supervision, and Compose's own named volumes (`vsms-dev_vsms_dev_pgdata`, `vsms-dev_vsms_dev_secrets`, both scoped to the `vsms-dev` Compose project — see `compose.dev.yaml`'s own header for why this file doesn't reuse `compose.yml`'s `vsms` project) own state:

```bash
docker compose -f compose.dev.yaml --profile console logs -f sms-gateway   # or sms-worker, sms-fake-orange, admin
docker compose -f compose.dev.yaml --profile console ps                     # same as `just demo-status`
```

`docker compose ... logs <service>` is where to look when something is wrong — a message that never leaves `accepted` is a worker problem (`logs sms-worker`), one stuck at `routed` is a fake-orange problem (`logs sms-fake-orange`).

The console's own machine client (`provision-client`, a real, HTTP-usable `private_key_jwt` credential — never gated behind `--profile console`, R4) and the human operator account both write their one-time secrets to their own container's log, never to a file on disk:

```bash
docker compose -f compose.dev.yaml logs provision-client   # the console's client id + where its key landed
docker compose -f compose.dev.yaml logs provision-user     # demo@vsms.local's generated password (just demo-login)
```

## Giving each developer their own credential

`just demo` provisions one client, for the console. Each service or developer should get their own against the same `App` (`vsms-demo`, the fixed slug `compose.dev.yaml`'s own `seed-demo-app` step uses):

```bash
docker compose -f compose.dev.yaml run --rm -v "$(pwd)/.e2e:/out" sms-gateway \
  provision-client --app-slug vsms-demo --label "billing service — alice" \
  --scope sms:send --scope sms:read \
  --key-out /out/alice-key.pem --client-id-out /out/alice-client-id
```

`--key-out` is written once at `0600` and the command refuses to overwrite an existing path — `-v "$(pwd)/.e2e:/out"` (a plain host bind mount, not the named secrets volume the compose stack's own services use, since `docker compose run --rm` removes its container immediately and there is no `docker compose cp` source afterward) lands the result somewhere you control. Point your application at `http://127.0.0.1:${VSMS_DEMO_GATEWAY_PORT:-8080}` with that client id and key — see [`../integrating.md`](../integrating.md).

## Proving the whole chain

```bash
just e2e-integration
```

Rebuilds the stack (`just demo`), provisions a *second* independent client as an "external integrator", sends through [`ci/e2e-integration`](../../ci/e2e-integration) (a small Rust tool, not `examples/rust/sms-send` — see that tool's own module doc for why it has to run *inside* the Compose network rather than as a host process against this stack specifically) over real HTTP, then polls `GET /messages/{id}` **as the console's own credential** until that exact message reaches `delivered`. It exits non-zero at the first broken link, naming the step. [`e2e-integration.md`](e2e-integration.md) explains what it proves and what it fakes.

`examples/rust/sms-send` is still the thing to hand someone who asks "what should my integration look like?" — it's an integrator-facing example, meant to run bare against a real, single-address gateway, which is exactly what it's for; it just can't reach `compose.dev.yaml`'s stack directly, for the reason `ci/e2e-integration`'s own module doc explains.

## Injecting failures

The happy path is the least interesting thing a fake carrier can do. Override `sms-fake-orange`'s own command to run it with a fault mode:

```bash
docker compose -f compose.dev.yaml stop sms-fake-orange
docker compose -f compose.dev.yaml run --rm --service-ports sms-fake-orange \
  --bind-addr 0.0.0.0:8090 \
  --dlr-endpoint http://sms-gateway:8080/dlr/orange_cm \
  --sender-number +2370000 \
  --fault-mode seeded --seed 3
```

(`stop` first, so the fault-mode `run` can bind the same service network alias the stopped container was using; `--service-ports` republishes `compose.dev.yaml`'s own loopback port mapping for it.)

| Flag | What it simulates |
|---|---|
| `--fault-mode seeded --seed <n>` | A reproducible weighted mix: rejections, 429s, post-accept timeouts, and duplicate, out-of-order, unknown-reference, or submit-racing delivery receipts. Same seed, same draw. |
| `--reject-tokens` | Carrier credentials revoked mid-flight — every submit fails `Permanent`. |
| `--dlr-delay-ms 30000` | Slow delivery, for testing your own timeouts and polling. |

The same weighted distribution drives [`crates/sms-worker/tests/chaos_live_postgres.rs`](../../crates/sms-worker/tests/chaos_live_postgres.rs), which asserts invariants rather than outcomes: no message lost, nothing left claimable, and a message that went `uncertain` never submitted twice.

## Running the tests

```bash
just check        # cargo check, with the build-concurrency cap
just test         # unit + in-process; the live suites stay ignored here
just test-live    # every live-Postgres suite — manages its own container
just all-checks   # everything CI runs, in CI's order
```

`test-live` starts (or reuses) one labelled Postgres container and gives each test binary its own database. **Do not run two `test-live` invocations concurrently on one machine** — the container name is global and they will corrupt each other's runs. `just test-live-clean` removes it.

## Known gaps

- **`just demo` is loopback-only.** Every service publishes only on `127.0.0.1`. Reaching it from another machine, a phone, or a container needs the port bindings in `compose.dev.yaml` changed.
- **The first build on a machine can hit a real Docker BuildKit cache-mount race.** `just demo`'s own recipe forces `COMPOSE_PARALLEL_LIMIT=1` for exactly this reason — see that recipe's own comment in the `justfile` for the mechanism (several `app/*/Dockerfile` builder stages deliberately share one cargo-registry cache-mount id, which several genuinely-first, uncached builds racing at once can corrupt). A cache already warmed by a previous `just demo` doesn't hit this.

## Troubleshooting

| Symptom | Cause |
|---|---|
| Gateway exits immediately at startup | No active OP signing key or no active `orange_cm` `Provider`/`Route` — both checked *before* the listener binds, so this is a startup failure, never a first-request one. `docker compose -f compose.dev.yaml logs seed-signing-key` / `logs seed-dispatch` show whether those one-shot steps actually completed. |
| Messages stay `accepted` | The worker isn't running, or isn't running the `dispatch` role. `docker compose -f compose.dev.yaml logs sms-worker`. |
| Messages reach `routed` and stop | `sms-fake-orange` isn't reachable at `ORANGE_CM_BASE_URL`. `docker compose -f compose.dev.yaml logs sms-fake-orange`. |
| Messages reach `submitted` and stop | The DLR never arrived — check `sms-gateway`'s own `ORANGE_CM_DLR_NOTIFY_URL`/`sms-worker`'s `ORANGE_CM_DLR_NOTIFY_URL` in `compose.dev.yaml` match the internal `sms-gateway:8080` address, not a host-published port. |
| `429` from everything | A rate-limit bucket. `docker compose -f compose.dev.yaml restart sms-gateway` to reset, or slow down. |
| Console login fails | The `sms-console` OIDC client's `redirect_uri` must match the console's port **exactly**; a changed `VSMS_DEMO_CONSOLE_PORT` needs a fresh `just demo`. |
| A hash-related failure after mixing runs | `SMS_HASH_PEPPER` differs between processes. Every service in `compose.dev.yaml` is pinned to the same hardcoded demo value in that file — nothing detects a mismatch if you've overridden one service's environment by hand and not another's. |
