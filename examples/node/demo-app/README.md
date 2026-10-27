# vsms demo app

The demo showcase's own evaluator — not another isolated integration example
like its two siblings (`../sms-send-example`, `../webhook-receiver`), but the
thing `compose.dev.yaml`/`compose.demo.yaml`'s own `demo-app` service actually
runs against the real stack those compose files bring up.

It does both halves at once, against real containers, over real HTTP:

1. Signs every request to `sms-gateway` with its own Ed25519 key and verifies
   every answer against the gateway's two pinned keys, via
   [`@vaam-apps/vsms-node`](https://www.npmjs.com/package/@vaam-apps/vsms-node) —
   the same SDK an external integrator would use (here built from this
   checkout, `file:../../../sdks/node/vsms-sdk-node`). The compose stacks'
   `register-demo-app` step registers the key's public half as the app's own
   `AppClient`; there is no token to acquire.
2. Sends one `otp`-class message.
3. Starts a small Express server on `:9000` (`POST /webhooks`) and polls
   `GET /messages/{id}` until the message reaches a terminal state, printing
   a timeline with wall-clock deltas as it goes.
4. Opens every inbound webhook with the SDK's `WebhookOpener`. A delivery is a
   sealed request signed with the gateway's own key (the one whose public half
   is in `server-keys.txt`), addressed to the audience the demo's
   `WebhookEndpoint` was created with, so there is no secret to hand this
   container and no signature header to check.

**Exit code is the whole point.** `0` only if the message reached `delivered`
**and** at least one webhook for it opened under the gateway's pinned key. Anything else is
a loud, non-zero failure that names exactly what didn't happen — meant to be
read from `docker compose logs demo-app` (or `just demo-app`), not just
trusted to have worked.

## Running it

Normally you don't run this by hand — `just demo`/`just demo-app` does, inside
`compose.dev.yaml`'s network, where `sms-gateway` and `sms-worker` reach
`demo-app` at its Compose-internal DNS name (`http://demo-app:9000/webhooks`,
exactly what `seed-demo-webhook` seeds).

**Running this package as a bare host process only proves the outbound
half** (the signed `sendMessage`, polling to a terminal
state) — **not** webhook delivery. `sms-worker`'s `hooks` role runs
*inside* the Compose network and has no route to a process listening on
the host's own loopback; nothing in `compose.dev.yaml` maps a host port
onto `hooks`' own outbound reachability, and `hooks` doesn't consult
`DEMO_WEBHOOK_AUDIENCE` or anything else this process's own environment
sets — it delivers to whatever URL is actually stored on the
`WebhookEndpoint` row, which is Compose-internal by construction. A host
run's webhook server starts and listens exactly as it does in the
container, but nothing seeded through `just demo` will ever reach it,
so it will sit at zero received events, waiting, until `DEMO_TIMEOUT_MS`
expires and it reports `FAILURE: ... no webhook was received at all`.
That failure is expected here, not a bug — treat a host run as a way to
exercise the send/auth/poll path in isolation (e.g. against a
differently-hosted gateway with its own real webhook wiring), not as a
substitute for `just demo-app`'s full proof. To run it standalone
against a `just demo` stack's outbound half from the host:

```bash
cd examples/node/demo-app
pnpm install --ignore-workspace --frozen-lockfile
VSMS_BASE_URL=http://127.0.0.1:8080 \
VSMS_SIGNING_KEY_PATH=../../../dev-keys/demo-app.pem \
VSMS_SERVER_KEYS_PATH=../../../dev-keys/server-keys.txt \
  pnpm start
```

(`just dev-keys` mints both files, and `just demo-up` registers the key.
Against another gateway, register your own key with `sms-gateway
register-client` and pin that gateway's `sms-gateway server-keys`.)

## Configuration

Every value has a default matching what `compose.dev.yaml`/`compose.demo.yaml`
pass, so nothing below is required inside those stacks:

| Variable                | Default                     | What it is                                                                                                                                                                                    |
| ----------------------- | --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `VSMS_BASE_URL`         | `http://sms-gateway:8080`   | The gateway's origin.                                                                                                                                                                         |
| `VSMS_SIGNING_KEY_PATH` | `/run/demo/demo-app.pem`    | The app's Ed25519 private key (PKCS#8 PEM); its public half is what `register-client` registered.                                                                                             |
| `VSMS_SERVER_KEYS_PATH` | `/run/demo/server-keys.txt` | The gateway's two public sealing keys, as `sms-gateway server-keys` prints them (`current:` / `next:`).                                                                                       |
| `DEMO_WEBHOOK_AUDIENCE` | `demo-app`                  | The audience the demo's `WebhookEndpoint` was created with (`vsms-demo-seed --webhook-audience`). A delivery is addressed to it; **pass a different value to prove the opener fails closed.** |
| `DEMO_WEBHOOK_PORT`     | `9000`                      | Port this process's own receiver listens on.                                                                                                                                                  |
| `DEMO_TO`               | `+237677123456`             | Recipient MSISDN.                                                                                                                                                                             |
| `DEMO_SENDER_ID`        | `VSMS`                      | Must match the `SenderId.value` `vsms-demo-seed` approved.                                                                                                                                    |
| `DEMO_BODY`             | *(a sample OTP body)*       | Message body.                                                                                                                                                                                 |
| `DEMO_TIMEOUT_MS`       | `90000`                     | Overall deadline — send retries, state polling, and the webhook settle period all share this one budget.                                                                                      |

## What "wrong audience" looks like

Pass a `DEMO_WEBHOOK_AUDIENCE` that doesn't match what `vsms-demo-seed` wrote
to the `WebhookEndpoint` row, and the message still reaches `delivered` (the
send/delivery path has no idea this receiver exists) but every received
webhook logs `[webhook] DELIVERY NOT OPENED`, the process reports zero verified
webhooks, and exits non-zero. `WebhookOpener` fails closed rather than
accepting whatever arrives.

## Tests

```bash
node --test
```

(`node --test` directly, not `pnpm test`: from a full checkout, pnpm 11
verifies dependencies before every `run` against the *root* workspace
lockfile, which does not know this standalone package, and refuses. The
`js` CI job invokes `tsc` and `node --test` the same way.)

`decision.test.ts`
covers all four (delivered × verified) quadrants of `decide()` — the one
function that turns a run's own observations into the process's exit code
— directly, with no server, no HTTP, no SDK.
