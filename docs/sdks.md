# SDKs

vsms ships two official SDKs, built from this repository and published with
every release. Both exist for the same reason: vsms has no tokens, so there
is no credential lifecycle to hand-roll. A request is a COSE_Sign1 message
signed with your Ed25519 key and bound to its route, the audience and a fresh
nonce; the answer is one signed by the gateway's key, which you pin. Your code
calls `send_message` / `sendMessage` and never touches a JWT.

You still need a key registered first: you make an Ed25519 key, an operator
runs `sms-gateway register-client` with its public half and gives you the
gateway's two public keys (`sms-gateway server-keys`).
[Integrating with vsms](integrating.md#1-get-a-credential) covers that step,
and everything the SDKs send on your behalf.

| Language | Package                                                                      | Source                                                  |
| -------- | ---------------------------------------------------------------------------- | ------------------------------------------------------- |
| Rust     | [`vsms-sdk-rust`](https://crates.io/crates/vsms-sdk-rust) on crates.io       | [`sdks/rust/vsms-sdk-rust`](../sdks/rust/vsms-sdk-rust) |
| Node.js  | [`@vaam-apps/vsms-node`](https://www.npmjs.com/package/@vaam-apps/vsms-node) | [`sdks/node/vsms-sdk-node`](../sdks/node/vsms-sdk-node) |

The two do not implement the envelope twice. The Node package carries
[`sdks/rust/vsms-sdk-wasm`](../sdks/rust/vsms-sdk-wasm), the same cratestack
`ClientEnvelope` the Rust SDK seals through, compiled to WebAssembly, and
`cargo xtask sdk-parity-check` holds the two to one audience and one operation
table (the wasm crate expands the Rust SDK's vendored schema).

Neither SDK retries a request, follows a redirect or believes an answer that is
not sealed: a retry is a new request with a new nonce, which is the caller's
decision, and a redirect would re-send the signed body to a host the signature
was not addressed to. A refused signature is one coarse unsigned `401` that
says nothing about which check failed (`SdkError::is_unauthorized` /
`SdkError.isUnauthorized()`); look at the registration of the key and at the
clock.

## Rust

The crate is `vsms-sdk-rust`; its library is imported as `vsms_sdk`.

```rust
use vsms_sdk::{ServerKeys, SigningKey, VsmsClient};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let key = SigningKey::from_pem_file("/path/to/vsms-signing-key.pem")?;
    // `sms-gateway server-keys`' output: public, so plain configuration.
    let server_keys = ServerKeys::parse_report(&std::fs::read_to_string("server-keys.txt")?)?;
    let client = VsmsClient::signed("http://127.0.0.1:8080", key, server_keys)?;

    let outcome = client
        .send_message(
            vsms_sdk::schema::SendMessageInput {
                to: "+237677123456".to_owned(),
                body: "Hello from vsms-sdk-rust".to_owned(),
                senderId: Some("VYMALO".to_owned()),
                class: None,
                clientRef: None,
                scheduledAt: None,
                validityMinutes: None,
            },
            // Optional `Idempotency-Key`: a retry under the same key
            // replays the first response instead of sending twice.
            Some("a caller-chosen retry-safe key"),
        )
        .await?;

    println!(
        "sent: {} ({}), replayed={}",
        outcome.result.messageId, outcome.result.state, outcome.idempotency_replayed
    );
    Ok(())
}
```

The models, inputs and procedure stubs are generated from the vsms schema by
CrateStack's `include_client_schema!`; the sealing of `send_message` is
hand-written. A complete, runnable version is
[`examples/rust/sms-send`](../examples/rust/sms-send), which runs against the
gateway `just demo` starts.

## Node.js

```bash
npm install @vaam-apps/vsms-node
# or
pnpm add @vaam-apps/vsms-node
```

```typescript
import { readFileSync } from "node:fs";
import { parseServerKeysReport, VsmsClient } from "@vaam-apps/vsms-node";

// Your key on disk (PKCS#8 PEM), and the gateway's two pinned public keys.
const client = new VsmsClient({
  baseUrl: "http://127.0.0.1:8080",
  keyPath: "/path/to/vsms-signing-key.pem",
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

The key can also be a `crypto.KeyObject`, the 32-byte seed, or a `signer`
callback for a key this process cannot read (a KMS); an answer sealed under
the pinned `next` key is trusted, so a rotation is not a flag day. Every
failure is an `SdkError` with a `kind` (`config`, `seal`, `transport`,
`unsigned`, `unverified`, `remote`, ...), and the client reports whether a send
was replayed under its `Idempotency-Key` (`idempotencyReplayed`); see the
[package README](../sdks/node/vsms-sdk-node/README.md).
[`examples/node/sms-send-example`](../examples/node/sms-send-example) is the
runnable version.

## `Idempotency-Key` and `clientRef` are different things

Both SDKs take an optional idempotency key, sent as the `Idempotency-Key`
header. It protects against **not knowing whether a request landed** (a
timeout, a dropped connection) by replaying the first response on a retry,
never sending twice. `clientRef` in the message input is a separate,
database-level dedupe scoped to your `App`, for **deliberately** sending the
same logical message twice. Both can come back as a `409`;
`SdkError::is_idempotency_in_flight` and `SdkError::is_conflict` (Rust),
`isIdempotencyInFlight()` and `isConflict()` (Node) tell them apart. See
[Idempotency](integrating.md#5-idempotency-idempotency-key-vs-clientref) in
the integration guide.

## Any other language

There is no SDK to wait for, but there is no shortcut either: a request is a
COSE_Sign1 envelope, which [Other languages](integrating.md#other-languages)
says how to build. To receive webhooks,
[`examples/node/webhook-receiver`](../examples/node/webhook-receiver) is a
reference receiver that verifies the signature scheme.

Publishing both packages is automated; the
[SDK release runbook](runbooks/releasing-sdks.adoc) covers the registries,
secrets and dry runs.
