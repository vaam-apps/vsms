# @vaam-apps/vsms-node

Official Node.js SDK for [vsms](https://github.com/vaam-apps/vsms).

Signs every request with your Ed25519 key and verifies every answer against the gateway's pinned public keys, so a caller writes `client.sendMessage(...)` and handles no token, no JWT and no session.

## Features

- **A key is the only credential.** Every request is a COSE_Sign1 message bound to its route, its audience, the called operation and a fresh nonce. The private half never reaches vsms; there is nothing to acquire, cache or refresh.
- **Every answer is verified.** An answer that is not sealed, or does not verify under one of the two pinned server keys, is an error whatever its status. The gateway's own refusals are one coarse unsigned `401` (`SdkError.isUnauthorized()`).
- **One attempt, no redirect.** A sealed request is never retried by the SDK (a retry is a new request with a new nonce, your decision) and a redirect is never followed (it would re-send the signed body to a host the signature was not addressed to). Answers are capped at 8 MiB.
- **Idempotency support.** `Idempotency-Key` is bound by the signature; `SendMessageOutcome.idempotencyReplayed` says whether the gateway replayed an earlier answer.
- **One implementation of the envelope.** The signing is the Rust one, compiled to WebAssembly and shipped inside this package (`wasm/`), so the Node and Rust SDKs cannot bind different bytes.

## Installation

```bash
npm install @vaam-apps/vsms-node
# or
pnpm add @vaam-apps/vsms-node
```

Node 22.11 or newer. The package carries its WebAssembly module inside its JavaScript (as base64), so it loads from a bundle without a `.wasm` file beside it. That is tested for Node itself loading the built package from a directory that holds no `.wasm` file; bundling it into a single file with a particular bundler (Next.js, esbuild) is **not** tested here.

## Get a credential

vsms has no API keys, no shared secrets and no tokens. Make a key, give the operator its public half, and pin the gateway's two public keys:

```bash
openssl genpkey -algorithm ed25519 -out vsms-signing-key.pem   # keep this; it never leaves you

# the operator registers the public half for your app ...
sms-gateway register-client --app <slug> --public-key <base64url> --scope sms:send --scope sms:read
# ... and gives you the gateway's keys, public and so plain configuration:
sms-gateway server-keys --envelope-signing-key-file current.pem --envelope-next-signing-key-file next.pem > server-keys.txt
```

`new VsmsClient({...}).publicKey` prints the base64url public half of whatever key you gave it. Pin **both** lines of `server-keys.txt` (`current` and `next`): a rotation then needs no flag day.

## Quickstart

```typescript
import { readFileSync } from "node:fs";
import { parseServerKeysReport, VsmsClient } from "@vaam-apps/vsms-node";

const client = new VsmsClient({
  baseUrl: "http://127.0.0.1:8080",
  keyPath: "/path/to/vsms-signing-key.pem", // or privateKey, or signer (below)
  serverKeys: parseServerKeysReport(readFileSync("server-keys.txt", "utf8")),
});

// Send an SMS
const outcome = await client.sendMessage(
  {
    to: "+237677123456",
    senderId: "VYMALO",
    body: "Hello from @vaam-apps/vsms-node!",
  },
  {
    // Optional retry-safe idempotency key
    idempotencyKey: "my-custom-order-id-123",
  },
);

console.log("Sent message:", outcome.result.messageId, "State:", outcome.result.state);
console.log("Was replayed from cache:", outcome.idempotencyReplayed);

// Read message status
const message = await client.getMessage(outcome.result.messageId);
console.log("Status:", message.state);
```

## The gateway's address

`baseUrl` is an `http:` or `https:` origin with an optional path prefix (`https://sms.example.com`, `https://example.com/sms/`). A query string, a fragment or credentials (`user:password@`) in it is a `config` error: each would let the URL a request goes to differ from the route the signature binds, and credentials would make Node add an `Authorization: Basic` header, which a keypair-only client never sends. Before a request is sent, the URL is checked against the target that was signed; an id of `.` or `..` is refused for the same reason.

## Keys

Give exactly one of:

| Option       | What it is                                                                                    | Where the key lives                         |
| ------------ | --------------------------------------------------------------------------------------------- | ------------------------------------------- |
| `keyPath`    | A file holding a PKCS#8 Ed25519 PEM (`openssl genpkey -algorithm ed25519`)                    | In Node, signing through `crypto.sign`      |
| `privateKey` | The same PEM as text, a `crypto.KeyObject`, or the 32-byte seed (bytes or unpadded base64url) | PEM and `KeyObject`: in Node. Seed: in wasm |
| `signer`     | `{ publicKey, sign(tbs) }` for a key this process cannot read (a KMS, an HSM)                 | Outside this process                        |

`sign` receives the exact bytes to sign and returns the 64-byte pure Ed25519 signature over them (`crypto.sign(null, tbs, key)`, never a pre-hash). A signature that does not verify under `publicKey` is refused before the request is sent, rather than failing at the gateway as the coarse `401`.

Only Ed25519 is an app client's key. A P-256 key is a *console* key, which a browser holds as a non-extractable WebCrypto key; the sealing module supports it for the operator console (`Sealer.fromEsp256Callback`), but this client does not offer it.

## Forwarding someone else's sealed request

A server that only carries another party's sealed request (the operator console forwards a browser's) holds no signing key and no sealed call to open the answer with, and must still not take a `200` from a socket for the gateway's word. `AnswerOpener` verifies the answer to the bytes that were forwarded, under the same two pinned keys:

```ts
import { AnswerOpener } from "@vaam-apps/vsms-node";

const opener = new AnswerOpener({ serverKeys });
const text = await opener.open(
  { method: "POST", target: "/$procs/authorizeComposition", body: forwardedBytes },
  { status, headers, body },
); // the JSON text, or an SdkError of kind `unsigned` / `unverified` / `unreadable`
```

`routeOf(method, path)` is the schema's route for a concrete path, or `undefined` for anything the gateway does not seal; a pass-through asks it before opening a connection.

## Receiving webhooks

vsms delivers an event to a sealed endpoint as a sealed request signed with the gateway's own key (the one that signs its answers): no webhook secret, no signature header. (An endpoint can also be set to the original `hmac` mode, `X-Sms-Signature` under a shared secret, for a receiver that has not moved; `WebhookOpener` opens sealed deliveries only, and an `hmac` receiver verifies the signature itself, `docs/integrating.md` "Choosing a delivery mode".) Pin the same two public keys and open the body:

```ts
import { parseServerKeysReport, WebhookOpener } from "@vaam-apps/vsms-node";

const opener = new WebhookOpener({
  serverKeys: parseServerKeysReport(readFileSync("server-keys.txt", "utf8")),
  audience: "https://hooks.example.com", // the name on your WebhookEndpoint: choose one only you can have
  route: "/webhooks/vsms", // the whole path of its URL
  appId: "capp00000000000000000", // your App's id (required)
  endpointId: "cendpoint0000000000000", // your WebhookEndpoint's id (required): the binding
});
// `rawBody` is the request's bytes (a Buffer), never text a framework decoded.
const event = await opener.open(rawBody); // { id, eventId, type, occurredAt, appId, endpointId, data }
```

One gateway key signs every app's deliveries, so a signature and an audience say only that vsms sent this to your name. **`endpointId` is the binding**: a receiver serves exactly one endpoint, whose id it learns when the endpoint is created (the console panel shows it), and `open` compares it with the `endpointId` inside the signed envelope. It is the one signed value nobody but the gateway chooses: whoever may create endpoints can register one under your `appId` and, while your audience name is free, give it that name. `appId` additionally catches an endpoint registered under another app, and the unique audience is defence in depth. A delivery made through someone else's endpoint is refused like a forgery. There is no way to build an opener without both ids (the constructor throws `config` on a missing or empty one).

`open` takes bytes only (`Uint8Array`/`Buffer`) and throws an `SdkError` of kind `config` for a string: re-encoding text a framework already decoded can change the bytes, and a signature cannot survive that. Read the body raw (`express.raw({ type: "*/*" })`, `await request.arrayBuffer()`).

`open` rejects with an `SdkError` of kind `unverified` for a forgery, a body sealed by a key you did not pin, a delivery made for another app or endpoint, another audience or route, a stale timestamp (300 s) and a replayed body, and one answer is all it gives: reply `401`. `decode` means it verified and is not the documented envelope; `backend` that the check itself failed (reply `5xx`; vsms retries). Dedupe on `event.id`: delivery is at-least-once and a retry is a new sealed request carrying the same id. **The nonce memory is per process and cannot be replaced** (the Rust SDK's `WebhookOpener::builder(..).with_nonce_store(..)` can; here the check runs in WebAssembly, which cannot await a JavaScript store): a body replayed within the 300 s window to another replica, or to this one after a restart, opens a second time, so behind several replicas the id is your defence. `examples/node/webhook-receiver` is a complete receiver.

## Errors

Every failure is an `SdkError` with a `kind`:

| `kind`               | Meaning                                                                                                                                                           |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `config`             | An option is missing or unusable. Thrown by the constructor, never by the first request.                                                                          |
| `seal`               | Refused **before anything was sent**: the signer failed or took longer than `timeoutMs`, an id does not fit its route, or the URL would not be the signed one.    |
| `transport`          | The request could not be sent or the answer not received (refused connection, timeout).                                                                           |
| `response_too_large` | The answer is longer than `MAX_RESPONSE_BYTES`.                                                                                                                   |
| `unsigned`           | The answer is not sealed. The gateway refusing your signature is this with `httpStatus` 401.                                                                      |
| `unverified`         | The answer is sealed but does not verify under a pinned key, or does not answer this request.                                                                     |
| `unreadable`         | The request **was sent**, and an answer arrived that could not be read. Whether the gateway acted is unknown: do not retry a write without its `Idempotency-Key`. |
| `remote`             | A sealed, verified answer with a non-2xx status (`gatewayCode`, `fieldErrors`, `isConflict()`, ...)                                                               |
| `decode`             | A sealed, verified 2xx answer whose body is not the expected JSON, or a verified webhook delivery that is not the documented envelope.                            |
| `backend`            | A webhook delivery could not be checked because the receiver's own machinery failed (answer `5xx`).                                                               |

`seal` is the only kind that means "nothing was sent"; every failure after the send is `transport`, `response_too_large`, `unsigned`, `unverified` or `unreadable`.

`timeoutMs` (default 30 s) is one deadline for the whole call, signing included: a `signer` that never answers (a hung KMS) ends the call with `seal` at the deadline, and the client signs the next call normally. A `signer` must return its signature as a `Uint8Array`; any other value (a string, a number, an `ArrayBuffer`) is a `seal` failure, not a crash.

`isUnauthorized()` means the gateway did not accept the signature. It is deliberately one answer for an unknown or revoked key, an inactive client, a stale clock (the gateway allows five minutes), a replayed request and a bad signature: check that the public half is registered and not revoked, and this machine's clock.

## Development

The signing is `sdks/rust/vsms-sdk-wasm`, compiled to WebAssembly by `scripts/build-wasm.sh`. Building needs Rust with the `wasm32-unknown-unknown` target and the `wasm-bindgen` command-line tool at exactly the version `sdks/rust/Cargo.lock` pins: from the repository root, `just wasm-tools` installs both, then:

```bash
pnpm --filter @vaam-apps/vsms-node run build      # wasm/ and dist/
pnpm --filter @vaam-apps/vsms-node run typecheck
pnpm --filter @vaam-apps/vsms-node run test       # builds wasm-test/ too, then scripts/run-tests.mjs
```

`pnpm test` runs against `wasm-test/`, the same crate with its `responder` feature on: cratestack's *server* envelope, so the suite verifies what the SDK seals with code that is not the SDK's. `test:live` runs `live/gateway.live.ts` against a real `sms-gateway serve` (see that file's header for how to start one).
