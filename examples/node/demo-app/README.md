# vsms demo app

The demo showcase's own evaluator — not another isolated integration example
like its two siblings (`../sms-send-example`, `../webhook-receiver`), but the
thing `compose.dev.yaml`/`compose.demo.yaml`'s own `demo-app` service actually
runs against the real stack those compose files bring up.

It does both halves at once, against real containers, over real HTTP:

1. Authenticates to `sms-gateway` with `private_key_jwt`, via
   [`@vymalo/vsms-node`](https://www.npmjs.com/package/@vymalo/vsms-node) —
   the same published SDK an external integrator would use, reusing the
   machine credential `provision-client` already provisions for the admin
   console (no second `AppClient` is provisioned just for this).
2. Sends one `otp`-class message.
3. Starts a small Express server on `:9000` (`POST /webhooks`) and polls
   `GET /messages/{id}` until the message reaches a terminal state, printing
   a timeline with wall-clock deltas as it goes.
4. Verifies every inbound webhook's `X-Sms-Signature` — via `signature.ts`,
   a byte-for-byte copy of `../webhook-receiver/src/signature.ts` (see
   `verbatim-copy.test.ts`, which fails loudly the moment the two drift).

**Exit code is the whole point.** `0` only if the message reached `delivered`
**and** at least one webhook for it verified its signature. Anything else is
a loud, non-zero failure that names exactly what didn't happen — meant to be
read from `docker compose logs demo-app` (or `just demo-app`), not just
trusted to have worked.

## Running it

Normally you don't run this by hand — `just demo`/`just demo-app` does, inside
`compose.dev.yaml`'s network, where `sms-gateway` and `sms-worker` reach
`demo-app` at its Compose-internal DNS name (`http://demo-app:9000/webhooks`,
exactly what `seed-demo-webhook` seeds).

**Running this package as a bare host process only proves the outbound
half** (`private_key_jwt` auth, `sendMessage`, polling to a terminal
state) — **not** webhook delivery. `sms-worker`'s `hooks` role runs
*inside* the Compose network and has no route to a process listening on
the host's own loopback; nothing in `compose.dev.yaml` maps a host port
onto `hooks`' own outbound reachability, and `hooks` doesn't consult
`DEMO_WEBHOOK_SECRET` or anything else this process's own environment
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
VSMS_ISSUER=http://127.0.0.1:8080 \
VSMS_CLIENT_ID_PATH=../../../.e2e/console-client-id \
VSMS_PRIVATE_KEY_PATH=../../../.e2e/console-client-key.pem \
DEMO_WEBHOOK_SECRET=<any placeholder — nothing will call this receiver> \
  pnpm start
```

(`just e2e-integration` already copies the console credential to `.e2e/` —
reuse those paths, or point at your own `provision-client` output.)

## Configuration

Every value has a default matching what `compose.dev.yaml`/`compose.demo.yaml`
pass, so nothing below is required inside those stacks:

| Variable | Default | What it is |
|---|---|---|
| `VSMS_ISSUER` | `http://sms-gateway:8080` | The OP/API origin (same host, per this deployment's own design — see `@vymalo/vsms-node`'s own `privateKeyJwt` doc comment). |
| `VSMS_SCOPE` | `sms:send sms:read` | Requested token scope. |
| `VSMS_CLIENT_ID` | *(unset)* | If set, used directly. Otherwise read from `VSMS_CLIENT_ID_PATH`. |
| `VSMS_CLIENT_ID_PATH` | `/secrets/console-client-id` | Where `provision-client --client-id-out` wrote the client id. |
| `VSMS_PRIVATE_KEY_PATH` | `/secrets/console-client-key.pem` | Where `provision-client --key-out` wrote the private key. |
| `DEMO_WEBHOOK_SECRET` | *(unset)* | If set, used directly as the signing secret to verify against — **this is the override to pass a deliberately wrong value with, to prove signature verification actually fails closed.** Otherwise read from `DEMO_WEBHOOK_SECRET_PATH`. |
| `DEMO_WEBHOOK_SECRET_PATH` | `/secrets/webhook-secret` | Where `vsms-demo-seed`'s `--webhook-secret-out` wrote the `WebhookEndpoint.secret` it created/found. |
| `DEMO_WEBHOOK_PREV_SECRET` | *(unset)* | An optional second secret accepted alongside `DEMO_WEBHOOK_SECRET`, per §4.4's rotation-overlap design (`sms-webhook`: a receiver is handed both the current and previous secret and accepts either). `vsms-demo-seed`'s own seeded `WebhookEndpoint` always has `prevSecret: None` today, so nothing in the stack drives this — it's wired for completeness, not exercised by `just demo`. |
| `DEMO_WEBHOOK_PORT` | `9000` | Port this process's own receiver listens on. |
| `DEMO_TO` | `+237677123456` | Recipient MSISDN. |
| `DEMO_SENDER_ID` | `VSMS` | Must match the `SenderId.value` `vsms-demo-seed` approved. |
| `DEMO_BODY` | *(a sample OTP body)* | Message body. |
| `DEMO_TIMEOUT_MS` | `90000` | Overall deadline — send retries, state polling, and the webhook settle period all share this one budget. |

## What "wrong secret" looks like

Pass a `DEMO_WEBHOOK_SECRET` that doesn't match what `vsms-demo-seed` actually
wrote to the `WebhookEndpoint` row, and the message still reaches `delivered`
(the send/delivery path has no idea this receiver exists) but every received
webhook logs `[webhook] SIGNATURE VERIFICATION FAILED`, the process reports
zero verified webhooks, and exits non-zero — proving `verifySignature` fails
closed rather than accepting whatever arrives.

## Tests

```bash
node --test
```

(`node --test` directly, not `pnpm test`: from a full checkout, pnpm 11
verifies dependencies before every `run` against the *root* workspace
lockfile, which does not know this standalone package, and refuses. The
`js` CI job invokes `tsc` and `node --test` the same way.)

`cross-language-vectors.test.ts` checks `verifySignature` against fixtures a
*third*, independent tool computed (`openssl dgst -sha256 -hmac` — see that
file's own header). `verbatim-copy.test.ts` checks that this package's copy
of `signature.ts`/`cross-language-vectors.test.ts` hasn't drifted from
`../webhook-receiver`'s own originals (a repo-checkout test — it isn't, and
can't be, run inside the built image, since the sibling directory it reads
is outside this package's own Docker build context). `decision.test.ts`
covers all four (delivered × verified) quadrants of `decide()` — the one
function that turns a run's own observations into the process's exit code
— directly, with no server, no HTTP, no SDK.
