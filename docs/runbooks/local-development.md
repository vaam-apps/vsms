# Runbook: local development without a carrier

How to run the whole system on a laptop — gateway, worker, admin console, and a fake Orange — with no Orange or MTN account and no SMS ever reaching a real handset.

This is the runbook for daily development, whether you are working on vsms or against it. [`getting-started.md`](getting-started.md) is the longer, first-time walkthrough that builds the same thing by hand, one step at a time, and explains what each step proves; read that one when something here fails and you need to know which link broke. If you are integrating an application, [`../integrating.md`](../integrating.md) is what you want next.

## The one command

```bash
just demo
```

From a cold start this takes a few minutes (it builds the workspace). It brings up, in order: a scratch Postgres container, both migrations, an OP signing key, an `App` + `Provider` + `SenderId`, a machine client, the `sms-console` OIDC client, a human operator account, `sms-fake-orange`, `sms-gateway`, `sms-worker` (`dispatch,scheduler,jobs`), and the admin console.

It finishes by printing the URLs and the login it just generated:

```text
admin console:  http://127.0.0.1:3100/
sign in with:   demo@vsms.local / <generated, printed once>
sms-gateway:    http://127.0.0.1:8080/
sms-fake-orange (NOT a real provider): http://127.0.0.1:8090/
postgres:       postgres://postgres:postgres@localhost:15433/vsms_demo
```

```bash
just demo-status   # what's running
just demo-down     # stop everything, remove only its own container
```

`just demo` resets the database on every `up`. It is meant to be re-run, not kept alive for days.

## Why there is no carrier

`sms-fake-orange` ([`app/sms-fake-orange`](../../app/sms-fake-orange)) impersonates Orange Cameroon's token and submit endpoints, and independently POSTs delivery receipts back to the gateway's `POST /dlr/{providerKey}` route. It is a **participant, not a response stub** — "the SMS never arrived" is the absence of a later callback, which a mock that only answers the submit call can never model.

Nothing in the gateway or the worker knows it exists. The only difference from production is `ORANGE_CM_BASE_URL`. Every other code path — routing, submission, DLR ingestion, the state machine, webhooks — is the one that runs in production.

**Never point a deployment at it.** It logs a `WARN` saying so on every start, and no compose file references it.

## What `just demo` leaves on disk

Everything lands in `.demo/` (gitignored):

| Path | What |
|---|---|
| `.demo/pepper` | The run's `SMS_HASH_PEPPER`. Every process must share it. |
| `.demo/console-client-key.pem` | The console's machine client private key. |
| `.demo/console-client-id`, `.demo/app-id` | Ids other scripts reuse. |
| `.demo/{gateway,worker,fake-orange,admin}.log` | Where to look when something is wrong. |
| `.demo/*.pid` | Used by `demo-status` / `demo-down`. |

The logs are the first place to check, in that order — a message that never leaves `accepted` is a worker problem, one stuck at `routed` is a fake-orange problem.

## Giving each developer their own credential

`just demo` provisions one client, for the console. Each service or developer should get their own against the same `App`:

```bash
DATABASE_URL=postgres://postgres:postgres@localhost:15433/vsms_demo \
SMS_HASH_PEPPER="$(cat .demo/pepper)" \
./target/debug/sms-gateway provision-client \
  --app-id "$(cat .demo/app-id)" \
  --label "billing service — alice" \
  --scope sms:send --scope sms:read \
  --key-out ~/.vsms/alice-key.pem
```

`--key-out` is written once at `0600` and the command refuses to overwrite an existing path. Point your application at `http://127.0.0.1:8080` with that client id and key — see [`../integrating.md`](../integrating.md).

## Proving the whole chain

```bash
just e2e-integration
```

Rebuilds the stack, provisions a *second* independent client as an "external integrator", sends through [`examples/rust/sms-send`](../../examples/rust/sms-send) over real HTTP, then polls `GET /messages/{id}` **as the console's own credential** until that exact message reaches `delivered`. It exits non-zero at the first broken link, naming the step. [`e2e-integration.md`](e2e-integration.md) explains what it proves and what it fakes.

This is the script to hand someone who asks "what should my integration look like?"

## Injecting failures

The happy path is the least interesting thing a fake carrier can do. Stop the fake and restart it with a fault mode:

```bash
kill "$(cat .demo/fake-orange.pid)"

./target/debug/sms-fake-orange \
  --bind-addr 127.0.0.1:8090 \
  --dlr-endpoint http://127.0.0.1:8080/dlr/orange_cm \
  --sender-number +2370000 \
  --fault-mode seeded --seed 3
```

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

- **No container image for `sms-fake-orange`.** [`deploy/docker-compose.yml`](../../deploy/docker-compose.yml) accepts `ORANGE_CM_BASE_URL` pointed at a fake, and its header reserves a `fake-orange` profile, but no Dockerfile exists for it (only gateway, worker, migrate, and admin have one). A **shared** team dev instance therefore needs that image and compose service built first; a per-developer `just demo` needs nothing.
- **`just demo` is loopback-only.** Every service binds `127.0.0.1`. Reaching it from another machine, a phone, or a container needs the bind addresses changed.

## Troubleshooting

| Symptom | Cause |
|---|---|
| Gateway exits immediately at startup | No active OP signing key (`rotate-signing-key`) or no active `orange_cm` `Provider` row (`seed-provider`). Both are checked *before* the listener binds, so this is a startup failure, never a first-request one. |
| Messages stay `accepted` | The worker isn't running, or isn't running the `dispatch` role. Check `.demo/worker.log`. |
| Messages reach `routed` and stop | `sms-fake-orange` isn't reachable at `ORANGE_CM_BASE_URL`. Check `.demo/fake-orange.log`. |
| Messages reach `submitted` and stop | The DLR never arrived — the fake's `--dlr-endpoint` doesn't match the gateway's actual port. |
| `429` from everything | A rate-limit bucket. Restart the gateway to reset, or slow down. |
| Console login fails | The `sms-console` OIDC client's `redirect_uri` must match the console's port **exactly**; a changed `VSMS_DEMO_CONSOLE_PORT` needs a fresh `just demo`. |
| A hash-related failure after mixing runs | `SMS_HASH_PEPPER` differs between processes. Every process in one stack must share `.demo/pepper` — nothing detects a mismatch. |
