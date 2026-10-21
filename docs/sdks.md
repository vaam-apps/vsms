# SDKs

vsms ships two official SDKs, built from this repository and published with
every release. Both exist for the same reason: they own the
`private_key_jwt` credential lifecycle (signing the client assertion,
exchanging it at `POST /token`, caching the access token and refreshing it on
a `401`), so your code calls `send_message` / `sendMessage` and never touches
a JWT.

You still need a client credential first: an operator runs
`sms-gateway provision-client` and hands you a client id and a private key.
[Integrating with vsms](integrating.md#1-get-a-credential) covers that step,
and everything the SDKs send on your behalf.

| Language | Package                                                                      | Source                                                  |
| -------- | ---------------------------------------------------------------------------- | ------------------------------------------------------- |
| Rust     | [`vsms-sdk-rust`](https://crates.io/crates/vsms-sdk-rust) on crates.io       | [`sdks/rust/vsms-sdk-rust`](../sdks/rust/vsms-sdk-rust) |
| Node.js  | [`@vaam-apps/vsms-node`](https://www.npmjs.com/package/@vaam-apps/vsms-node) | [`sdks/node/vsms-sdk-node`](../sdks/node/vsms-sdk-node) |

## Rust

The crate is `vsms-sdk-rust`; its library is imported as `vsms_sdk`.

```rust
use vsms_sdk::{PrivateKeyJwtConfig, VsmsClient};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = PrivateKeyJwtConfig::from_key_path(
        "http://127.0.0.1:8080",
        "the client id provision-client printed",
        "/path/to/console-client-key.pem",
        "sms:send sms:read",
    )?;
    let client = VsmsClient::private_key_jwt("http://127.0.0.1:8080", config)?;

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
CrateStack's `include_client_schema!`; the auth layer and the bounded
refresh-on-`401` are hand-written. A complete, runnable version is
[`examples/rust/sms-send`](../examples/rust/sms-send), which runs against the
gateway `just demo` starts.

## Node.js

```bash
npm install @vaam-apps/vsms-node
# or
pnpm add @vaam-apps/vsms-node
```

```typescript
import { VsmsClient } from "@vaam-apps/vsms-node";

// Initialize client with private key on disk or PEM string
const client = VsmsClient.privateKeyJwt({
  issuer: "http://127.0.0.1:8080",
  clientId: "your-client-id",
  keyPath: "/path/to/client-key.pem",
  // scope defaults to "sms:send sms:read"
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

Besides signing and caching, the client de-duplicates concurrent token
requests, renews the token and retries once if it is invalidated mid-flight,
and reports whether a send was replayed under its `Idempotency-Key`
(`idempotencyReplayed`).
[`examples/node/sms-send-example`](../examples/node/sms-send-example) and the
demo's evaluator, [`examples/node/demo-app`](../examples/node/demo-app), both
use it.

## `Idempotency-Key` and `clientRef` are different things

Both SDKs take an optional idempotency key, sent as the `Idempotency-Key`
header. It protects against **not knowing whether a request landed** (a
timeout, a dropped connection) by replaying the first response on a retry,
never sending twice. `clientRef` in the message input is a separate,
database-level dedupe scoped to your `App`, for **deliberately** sending the
same logical message twice. Both can come back as a `409`; in Rust,
`SdkError::is_idempotency_in_flight` and `SdkError::is_conflict` tell them
apart. See [Idempotency](integrating.md#5-idempotency-idempotency-key-vs-clientref)
in the integration guide.

## Any other language

There is no SDK to wait for: the exchange is four HTTP calls, listed under
[Any language — raw HTTP](integrating.md#any-language--raw-http). To receive
webhooks, [`examples/node/webhook-receiver`](../examples/node/webhook-receiver)
is a reference receiver that verifies the signature scheme.

Publishing both packages is automated; the
[SDK release runbook](runbooks/releasing-sdks.adoc) covers the registries,
secrets and dry runs.
