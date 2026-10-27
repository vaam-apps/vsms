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

Node 22.11 or newer. The package carries its WebAssembly module inside its JavaScript (as base64), so it loads from a bundle (Next.js, esbuild) without a `.wasm` file beside it.

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

## Keys

Give exactly one of:

| Option       | What it is                                                                                    | Where the key lives                         |
| ------------ | --------------------------------------------------------------------------------------------- | ------------------------------------------- |
| `keyPath`    | A file holding a PKCS#8 Ed25519 PEM (`openssl genpkey -algorithm ed25519`)                    | In Node, signing through `crypto.sign`      |
| `privateKey` | The same PEM as text, a `crypto.KeyObject`, or the 32-byte seed (bytes or unpadded base64url) | PEM and `KeyObject`: in Node. Seed: in wasm |
| `signer`     | `{ publicKey, sign(tbs) }` for a key this process cannot read (a KMS, an HSM)                 | Outside this process                        |

`sign` receives the exact bytes to sign and returns the 64-byte pure Ed25519 signature over them (`crypto.sign(null, tbs, key)`, never a pre-hash). A signature that does not verify under `publicKey` is refused before the request is sent, rather than failing at the gateway as the coarse `401`.

Only Ed25519 is an app client's key. A P-256 key is a *console* key, which a browser holds as a non-extractable WebCrypto key; the sealing module supports it for the operator console (`Sealer.fromEsp256Callback`), but this client does not offer it.

## Errors

Every failure is an `SdkError` with a `kind`:

| `kind`               | Meaning                                                                                             |
| -------------------- | --------------------------------------------------------------------------------------------------- |
| `config`             | An option is missing or unusable. Thrown by the constructor, never by the first request.            |
| `seal`               | The call could not be signed, or the answer could not be read. Nothing was sent / believed.         |
| `transport`          | The request could not be sent or the answer not received (refused connection, timeout).             |
| `response_too_large` | The answer is longer than `MAX_RESPONSE_BYTES`.                                                     |
| `unsigned`           | The answer is not sealed. The gateway refusing your signature is this with `httpStatus` 401.        |
| `unverified`         | The answer is sealed but does not verify under a pinned key, or does not answer this request.       |
| `remote`             | A sealed, verified answer with a non-2xx status (`gatewayCode`, `fieldErrors`, `isConflict()`, ...) |
| `decode`             | A sealed, verified 2xx answer whose body is not the expected JSON.                                  |

`isUnauthorized()` means the gateway did not accept the signature. It is deliberately one answer for an unknown or revoked key, an inactive client, a stale clock (the gateway allows five minutes), a replayed request and a bad signature: check that the public half is registered and not revoked, and this machine's clock.

## Development

The signing is `sdks/rust/vsms-sdk-wasm`, compiled to WebAssembly by `scripts/build-wasm.sh`. Building needs Rust with the `wasm32-unknown-unknown` target and the `wasm-bindgen` command-line tool at exactly the version `sdks/rust/Cargo.lock` pins: from the repository root, `just wasm-tools` installs both, then:

```bash
pnpm --filter @vaam-apps/vsms-node run build      # wasm/ and dist/
pnpm --filter @vaam-apps/vsms-node run typecheck
pnpm --filter @vaam-apps/vsms-node run test       # builds wasm-test/ too, then node --test
```

`pnpm test` runs against `wasm-test/`, the same crate with its `responder` feature on: cratestack's *server* envelope, so the suite verifies what the SDK seals with code that is not the SDK's. `test:live` runs `live/gateway.live.ts` against a real `sms-gateway serve` (see that file's header for how to start one).
