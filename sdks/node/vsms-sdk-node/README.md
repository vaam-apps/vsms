# @vsms/sdk

Official Node.js SDK for [vsms](https://github.com/vymalo/vsms).

Owns the `private_key_jwt` credential lifecycle, so a caller writes `client.sendMessage(...)` and never touches a JWT.

## Features

- **Automated `private_key_jwt` Auth**: Automatic assertion signing (RFC 7523), token acquisition, in-memory caching with safety margins, and single-flight de-duplication.
- **Bounded 401 Refresh**: Transparently renews the access token and retries once if a token is invalidated mid-flight.
- **Idempotency Support**: Built-in support for `Idempotency-Key` headers and response replay tracking (`SendMessageOutcome.idempotencyReplayed`).
- **Standard TypeScript Types**: Clean domain types with zero framework lock-in.

## Installation

```bash
npm install @vsms/sdk jose
# or
pnpm add @vsms/sdk jose
```

## Quickstart

```typescript
import { VsmsClient } from "@vsms/sdk";

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
    body: "Hello from @vsms/sdk!",
  },
  {
    // Optional retry-safe idempotency key
    idempotencyKey: "my-custom-order-id-123",
  }
);

console.log("Sent message:", outcome.result.messageId, "State:", outcome.result.state);
console.log("Was replayed from cache:", outcome.idempotencyReplayed);

// Read message status
const message = await client.getMessage(outcome.result.messageId);
console.log("Status:", message.state);
```
