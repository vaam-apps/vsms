# vsms reference webhook receiver

A minimal Express receiver showing what a *correct* handler of vsms delivery
webhooks looks like — the inbound half of a vsms integration. The outbound
half (authenticating and sending) is [#149](https://github.com/vymalo/vsms/issues/149).

## Two delivery modes

A `WebhookEndpoint` is delivered to in one of two modes (`deliveryMode`), and this
receiver serves exactly one, named by `WEBHOOK_RECEIVER_MODE`:

- **`sealed`** (the default, and the recommended one): a request signed with the
  gateway's own key, opened with the Node SDK's `WebhookOpener`. Nothing secret
  is configured. Most of this file describes it.
- **`hmac`**: the original `X-Sms-Signature` scheme, signed with a secret shared
  with the endpoint, for a receiver that has not moved to the seal yet. It is a
  legacy mode that a deployment can switch off (`auth.methods`, VSA-1). `src/signature.ts`
  verifies it, `src/hmac-opener.ts` fits it to the same handler, and
  [its section below](#hmac-mode) says how to configure it.

Everything else (the fast ack, the off-path work, dedupe on the event's `id`, the
out-of-order tolerance, the `400`/`401`/`500` mapping) is the same for both: the
mode only decides how a delivery is checked.

## A sealed delivery is a sealed request

For a `sealed` endpoint there is no webhook secret and no signature header. vsms's
worker **seals** each delivery: the body is a `COSE_Sign1` message signed with the
gateway's own envelope signing key (the key that signs its answers), bound to
the receiver's audience, `POST`, the route (the whole path of the endpoint's
URL), the delivery contract, a fresh nonce and a timestamp. A receiver pins the two public keys it already pins to open
vsms's answers (`sms-gateway server-keys` prints them) and opens the body with
[`@vaam-apps/vsms-node`](https://www.npmjs.com/package/@vaam-apps/vsms-node)'s
`WebhookOpener`:

```ts
const opener = new WebhookOpener({
  serverKeys: parseServerKeysReport(readFileSync("server-keys.txt", "utf8")),
  audience: "https://hooks.example.com", // the audience the WebhookEndpoint was created with
  route: "/webhooks/vsms", // the whole path of the endpoint's URL
  appId: "capp00000000000000000", // the vsms app the endpoint belongs to
  endpointId: "cend00000000000000000", // the endpoint itself: the console shows it when it is created
});
const event = await opener.open(rawBody); // throws SdkError kind "unverified" on anything wrong
```

`config.ts` reads these from the environment: `WEBHOOK_RECEIVER_AUDIENCE`,
**`WEBHOOK_RECEIVER_ENDPOINT_ID` and `WEBHOOK_RECEIVER_APP_ID` (both required: the
receiver refuses to start without either)**. One gateway key signs every app's
deliveries, so a signature and an audience say only that vsms sent this to your
name. **The `endpointId` is the binding**: a receiver serves exactly one
endpoint, learns its id when the endpoint is created (the console's panel shows
it) and the opener compares it with the `endpointId` inside the signed envelope,
the one signed value no one but the gateway chooses. Anyone who may create
endpoints can register one under your app id, so `appId` alone is not enough; it
additionally catches an endpoint registered under another app. The audience being
unique to one endpoint is defence in depth, not the binding: the name is free
before your endpoint exists, after it is deleted or renamed, and while someone
squats it. Choose an audience only you can have (your origin).

`event` is `{ id, eventId, type, occurredAt, appId, endpointId, data }`
(`docs/architecture.md` §8.4). `id` is the dedupe key; `eventId` is the source event's id, which used to
travel in the `X-Sms-Event-Id` header and now rides inside the sealed body,
because a header is no part of a seal. A sealed delivery sends none of the
`X-Sms-*` headers.

What `open` refuses, with one answer (`unverified`, reply `401`) that never says
which: a body not signed by a pinned key, a delivery addressed to another audience,
another route, another contract, a delivery made through **another endpoint**
(of your app or not), a delivery made for **another app**, a stale timestamp (±300 s),
a **replayed** body (the nonce is remembered) and a flipped bit. `open` takes the
raw request bytes only (`express.raw`), never text a framework decoded. The nonce
memory is per process and cannot be replaced (the check runs in WebAssembly);
behind several replicas, dedupe on `event.id` as well, which at-least-once
delivery asks of every receiver anyway.

A retry is a **new** sealed request (new nonce, new timestamp) carrying the same
`id`: the nonce stops a captured request being replayed, it is not the dedupe
key. An operator's replay does the same.

## Verified against a live sender

`backends/apps/sms-worker/tests/hooks_node_receiver_live.rs`
([#44](https://github.com/vymalo/vsms/issues/44)'s acceptance gate) spawns a
genuinely separate `sms-worker --roles hooks` process and a genuinely separate
`node src/gate-receiver.ts` process, with a real loopback TCP connection between
them and a real Postgres-backed `WebhookAttempt` row claimed through the real CAS
claim loop. It runs this directory's actual code. The opener is cratestack's
envelope compiled to WebAssembly; the worker's sealer is `sms-webhook` over the
same cratestack crates called natively. A second case seeds the endpoint with the
wrong audience and watches the receiver refuse it, a third seeds an endpoint of
*another app* that names the receiver's own audience and watches the receiver refuse
that too, and a fourth seeds a *second endpoint of the receiver's own app* and
watches the receiver refuse its delivery, so a regression on either side fails
the gate in both directions. Three more run the `hmac` mode the same way: a
delivery signed with the endpoint's secret is verified, one inside a rotation
window verifies at a receiver that holds only the old secret, and a receiver
holding another secret refuses it.

## Layout

```text
src/
  index.ts          entry point: a sealed receiver (the committed gateway vector, and a tampered copy) and an hmac one (signed locally, rotation window and a forgery)
  config.ts         builds the opener from the environment, for the mode WEBHOOK_RECEIVER_MODE names: pinned keys, audience, ids (sealed) or the secret (hmac)
  hmac-opener.ts    the hmac mode's opener: signature.ts fitted to the same seam as the SDK's WebhookOpener
  signature.ts      the hmac mode's verifier: a from-scratch transcription of the scheme (see its own doc comment)
  cross-language-vectors.test.ts  proves signature.ts against fixture signatures backends/crates/sms-webhook computed with openssl, not with either language
  server.ts         the Express app: raw-body capture, WebhookOpener, fast ack, off-path processing
  receiver.test.ts  the receiver against the real opener and the vector, and against a stub opener for dedupe and ordering
  store.ts          in-memory idempotency (primary: the event id; secondary: aggregateId+eventType) + out-of-order-tolerant state tracking
  work-queue.ts     a tiny off-request-path queue (what "work off the request path" means, made concrete)
  gate-receiver.ts  the receiver #44's live gate spawns, plus a test-only /__test__/results route
  types.ts          what the receiver reports about each request
```

## Running it

Requires Node with native TypeScript support unflagged (Node ≥23.6; this repo's
own `.nvmrc` pins Node 26, which has it — no `ts-node`/`tsx` dependency needed).
The package depends on the Node SDK by path (`file:../../../sdks/node/vsms-sdk-node`),
so **build the SDK first** (`pnpm --filter @vaam-apps/vsms-node run build` from the
repo root, which needs the wasm toolchain — `just wasm-tools`), then:

```bash
cd examples/node/webhook-receiver
pnpm install --ignore-workspace --frozen-lockfile
node --test   # the receiver against the real opener and the gateway's own vector
pnpm start    # the demo below
```

`pnpm start` boots the receiver on `http://localhost:4790` (override with
`WEBHOOK_RECEIVER_PORT`), posts the committed vector two ways, prints a
summary and exits. Nothing here binds port 3000, 8080, 8090, or 3100.

## What it demonstrates, live

1. **A genuine delivery.** Bytes the gateway's own `sms-webhook` crate sealed
   (`backends/crates/sms-webhook/tests/sealed_vector.rs` writes them, signed by a
   throwaway test key) are opened, acked with `202` and processed off the request
   path.
2. **A tampered body.** One flipped bit: `401`.

A recording cannot show a **replay** being refused: the nonce memory expires an
entry against the real clock, and the demo pins the opener's clock at the
recording's own timestamp, far behind it. The replay is proved where a delivery
is sealed afresh: `sdks/node/vsms-sdk-node/test/webhook.test.ts` and
`sdks/rust/vsms-sdk-rust/tests/webhook.rs`.

`receiver.test.ts` adds what needs events the test chooses: a retry (same `id`)
acked but not reprocessed, `message.delivered` arriving before
`message.submitted`, a wrong audience, a stale timestamp, a body that is not a
COSE message, an opener that refuses, and the `400`/`401`/`500` mapping.

## Configuring a real receiver

Everything is public; none of it is a secret.

| Setting                                              | What it is                                                                                                       |
| ---------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `WEBHOOK_RECEIVER_SERVER_KEYS_PATH`                  | The file `sms-gateway server-keys` printed (`current:` / `next:`) ...                                            |
| `WEBHOOK_RECEIVER_SERVER_CURRENT`, `..._SERVER_NEXT` | ... or the same two keys, unpadded base64url.                                                                    |
| `WEBHOOK_RECEIVER_AUDIENCE`                          | The audience the `WebhookEndpoint` was created with (default `webhook-receiver-example`).                        |
| the endpoint URL's path                              | Must be `/webhooks/vsms`: the delivery's route is the path of `WebhookEndpoint.url`, with no query or `{param}`. |
| `WEBHOOK_RECEIVER_MODE`                              | `sealed` (default) or `hmac`; anything else refuses to start.                                                    |

A receiver that cannot name the gateway's keys refuses to start: there is no
mode that skips them.

## hmac mode

For an endpoint whose `deliveryMode` is `hmac`, run with `WEBHOOK_RECEIVER_MODE=hmac`
and:

| Setting                        | What it is                                                                                                                            |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------- |
| `WEBHOOK_RECEIVER_SECRET`      | The endpoint's `secret`. **A secret**: read it from where you keep secrets. Required.                                                 |
| `WEBHOOK_RECEIVER_PREV_SECRET` | The endpoint's `prevSecret`, set only while a rotation's overlap window is open, so a delivery signed with either verifies. Optional. |

The scheme is unchanged since before sealed mode existed: four headers
(`X-Sms-Event`, `X-Sms-Event-Id`, `X-Sms-Timestamp`, `X-Sms-Signature`), the
four-field body `{id, type, occurredAt, data}`, and HMAC-SHA256 over
`v1\n{timestamp}\n{eventId}\n{sha256(body)}` with the **raw bytes** in the hash.
`signature.ts` is the verifier; its doc comment is the long story of how it was
confirmed (three independent implementations, one of them `openssl`), and
`cross-language-vectors.test.ts` is that proof, run on every push.

The dedupe key is the signed `X-Sms-Event-Id` (`HmacOpener` reports it as the
event's `id`). Two things this mode does not have, because the scheme cannot: no
app or endpoint to compare (the secret is the endpoint's own), and no freshness
window (`signature.ts` explains why dedupe is what stops a replay being acted on
twice). A receiver that can run the opener should prefer `sealed`.

## `src/gate-receiver.ts` — the `#44` live gate

The exact same `createReceiver` code, started with no demo attached and one
test-only diagnostic route (`GET /__test__/results`) so a driving test can
observe what it did over HTTP rather than scraping stdout. Point a real
`sms-worker --roles hooks` process at it (a `WebhookEndpoint.url` of
`http://<host>:<port>/webhooks/vsms` and the matching audience, the worker holding
the gateway's signing key; or an `hmac` endpoint, with this process started in hmac
mode with the endpoint's secret) and it opens a real, live delivery.

## What this does and doesn't prove

**Does prove:** the receiver's handling of duplicates, out-of-order arrival and
refusal is correct over real HTTP; that the SDK's opener opens exactly what the
gateway's sealer produces (the vector); and that a real worker's delivery opens
in a separate process.

**Does not prove:** retry timing (§8.5's 1s, 5s, 25s, 2m, 10m, 1h, 6h, 24h), or
anything about real-world network conditions; the gate is loopback-only.
