# Integrating with vsms

For a developer whose application sends SMS **through** vsms. If you are working *on* vsms itself, start with [`CONTRIBUTING.md`](../CONTRIBUTING.md) and [`architecture.md`](architecture.md) instead; if you want the whole stack running on your laptop, [`runbooks/local-development.adoc`](runbooks/local-development.adoc) is the shorter path and this guide is what you read next.

Everything below works against a local stack with no Orange or MTN account, no real handset, and no SMS ever leaving the machine. The signing key, the signed requests, the REST calls, and the webhooks are all real — only the carrier is faked.

## 1. Get a credential

vsms has no API keys, no shared secrets and no access tokens. A caller is an **`AppClient`** holding an Ed25519 private key, and every request is a COSE_Sign1 message signed with it. The gateway answers with a message signed by its own key, which you pin.

Make your key, then give the operator its public half:

```bash
openssl genpkey -algorithm ed25519 -out vsms-signing-key.pem
cargo run -p sms-send -- --print-public-key --signing-key-path vsms-signing-key.pem   # examples/rust/sms-send
# or, from Node: node src/index.mjs --print-public-key --signing-key-path vsms-signing-key.pem   # examples/node/sms-send-example
```

The operator registers it for your `App` and gives you the gateway's two public keys:

```bash
sms-gateway register-client --app <App.slug> --public-key <what you printed> \
  --scope sms:send --scope sms:read
sms-gateway server-keys --envelope-signing-key-file ... --envelope-next-signing-key-file ... > server-keys.txt
```

The private key never leaves you and is never stored server-side. If you lose it, the operator registers a new one and revokes the old; there is no recovery. Pin **both** lines of `server-keys.txt` (`current` and `next`): a rotation then needs no flag day.

Give each service (and ideally each developer) its own client, with `register-client --client-name <name>`, against the same `App`. Messages are visible per-`App`, not per-client, so two clients on one `App` can read each other's messages - see [`runbooks/e2e-integration.adoc`](runbooks/e2e-integration.adoc) for why that is the intended design.

## 2. Send

### Rust - the SDK

[`vsms-sdk-rust`](../sdks/rust/vsms-sdk-rust/README.md) seals every call and verifies every answer. You never touch a COSE message.

```rust
let key = SigningKey::from_pem_file("./vsms-signing-key.pem")?;
let server_keys = ServerKeys::parse_report(&std::fs::read_to_string("server-keys.txt")?)?;
let client = VsmsClient::signed("http://127.0.0.1:8080", key, server_keys)?;

let outcome = client.send_message(SendMessageInput {
    to: "+237677123456".to_owned(),
    body: "Your code is 4471".to_owned(),
    senderId: Some("VYMALO".to_owned()),
    class: None, clientRef: None, scheduledAt: None, validityMinutes: None,
}, Some("retry-safe-key-1")).await?;
```

Runnable: [`examples/rust/sms-send`](../examples/rust/sms-send).

### Node - the SDK

[`@vaam-apps/vsms-node`](../sdks/node/vsms-sdk-node/README.md) does the same from Node 22.11 or newer. The signing is the Rust one, compiled to WebAssembly and shipped inside the package, so the two SDKs cannot bind different bytes.

```typescript
import { readFileSync } from "node:fs";
import { parseServerKeysReport, VsmsClient } from "@vaam-apps/vsms-node";

const client = new VsmsClient({
  baseUrl: "http://127.0.0.1:8080",
  keyPath: "./vsms-signing-key.pem",
  serverKeys: parseServerKeysReport(readFileSync("server-keys.txt", "utf8")),
});

const outcome = await client.sendMessage(
  { to: "+237677123456", body: "Your code is 4471", senderId: "VYMALO" },
  { idempotencyKey: "retry-safe-key-1" },
);
```

Runnable: [`examples/node/sms-send-example`](../examples/node/sms-send-example/src/index.mjs). A failure is an `SdkError` whose `kind` says which part failed; `isUnauthorized()` is the gateway's coarse unsigned `401`. The SDK sends each request once, follows no redirect and reads at most 8 MiB of answer.

### Other languages

The operator console's own gateway package still speaks the removed token protocol until it signs (VS-5b), and `examples/node/demo-app` still depends on an older, token-era release of the Node SDK. There is no compatibility mode to fall back on.

To integrate from another language today, implement the signed transport directly: a COSE_Sign1 envelope (Ed25519, `application/json` payloads only) bound to the method, route, audience `vsms`, the called operation's contract digest, a fresh nonce and the issued-at time, and verification of the answer against the pinned server keys. The wire contract is cratestack's signed transport (its ADR 0006); [`backends/crates/sms-api/src/envelope.md`](../backends/crates/sms-api/src/envelope.md) says what vsms adds to it. `sms-gateway routes` prints the full route table and needs no database.

## 3. Request and response

`SendMessageInput` ([`schema.cstack`](../schemas/vsms.cstack)):

| Field             | Type                                                      | Notes                                                                                                         |
| ----------------- | --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `to`              | String, required                                          | E.164, `+237…`. Validated and normalised; a non-Cameroon or unallocated number is refused before persistence. |
| `body`            | String, required                                          | Encoding is analysed, not assumed — see below.                                                                |
| `senderId`        | String?                                                   | Must be a registered, approved sender id for your `App`.                                                      |
| `class`           | `otp` \| `transactional` \| `notification` \| `marketing` | Drives routing and quiet-hours rules.                                                                         |
| `clientRef`       | String?                                                   | **Your** dedupe key, scoped to the `App`. See §5.                                                             |
| `scheduledAt`     | DateTime?                                                 | Send later.                                                                                                   |
| `validityMinutes` | Int?                                                      | After this, the message expires rather than delivering.                                                       |

`SendMessageResult`:

```json
{
  "messageId": "c4f2a1b3d4e5f60718293a4b5",
  "state": "accepted",
  "encoding": "gsm7",
  "segments": 1,
  "operator": "orange",
  "estimatedCostXaf": "22.00"
}
```

`state` is always `accepted` on a successful send — delivery is asynchronous. Do not treat a 200 as delivery.

**`encoding` and `segments` are worth reading on every response.** A single character outside GSM 03.38 flips the whole body to UCS-2 and roughly halves the per-segment capacity — an accented capital (`È`, `Ù`) or an emoji is enough. `previewMessage` computes this without sending, if you want to check before committing. See [`backends/crates/sms-encoding`](../backends/crates/sms-encoding).

### `class` changes what is allowed, not just how it routes

Cameroonian law (Law No. 2024/017) and this system's own policy make `class` load-bearing. Getting it wrong means a rejected send, not a mis-tagged one:

| Class           | Consent record required | Quiet hours              |
| --------------- | ----------------------- | ------------------------ |
| `otp`           | No                      | None                     |
| `transactional` | No                      | None                     |
| `notification`  | **Yes**                 | None                     |
| `marketing`     | **Yes**                 | **08:00–20:00 WAT only** |

- **Consent** — sending `notification` or `marketing` to a recipient with no standing `ConsentRecord` for that class fails with a validation error naming the statute. An operator records consent; you cannot self-serve it.
- **Quiet hours** — a `marketing` send is refused outside 08:00–20:00 WAT. The window is checked against the **delivery** time, so `scheduledAt` cannot be used to land one at 22:00. Schedule it inside the window instead.
- **Opt-out** is enforced for every class, and is checked before the message is persisted.

Declaring `otp` on marketing traffic to dodge these checks is not prevented by the code — the declared class is recorded, not verified. It is your legal exposure, not a loophole.

## 4. Message states

What a caller actually observes, in the order they normally occur:

| State         | Terminal | Meaning                                                          |
| ------------- | -------- | ---------------------------------------------------------------- |
| `accepted`    |          | Persisted and validated. Your send succeeded.                    |
| `queued`      |          | Claimed by a worker for routing.                                 |
| `routed`      |          | A provider was selected; submission is in flight.                |
| `submitted`   |          | The carrier accepted it. Awaiting a delivery receipt.            |
| `delivered`   | ✓        | The carrier confirmed handset delivery.                          |
| `failed`      | ✓        | Permanently failed. `stateReason` says why.                      |
| `expired`     | ✓        | Passed `validityMinutes` without delivering.                     |
| `rejected`    | ✓        | Refused — opt-out, unapproved sender id, invalid recipient.      |
| `cancelled`   | ✓        | Cancelled via `cancelMessage` before it went out.                |
| `uncertain`   |          | The submit timed out **after** the carrier may have accepted it. |
| `undelivered` |          | A retryable carrier failure.                                     |

Two of these need care, and both are deliberate rather than accidental:

- **`uncertain` is never resubmitted.** When a submit's outcome is genuinely unknown, vsms chooses a possibly-lost message over a possibly-duplicated one. For OTP traffic that is the right trade — a user re-requests a code far more cheaply than they get two. A background job eventually resolves the message to a terminal state.
- **`undelivered` currently has no retry driver** ([#122](https://github.com/vymalo/vsms/issues/122)). A message that receives exactly one retryable-failure receipt and no follow-up stays there. If your application needs a retry, implement it on your side against `clientRef` — do not assume vsms will re-send.

## 5. Idempotency: `Idempotency-Key` vs `clientRef`

Two different layers protecting against two different problems. Use both.

- **`Idempotency-Key`** (HTTP header) — protects against *not knowing whether your request landed*: a timeout, a dropped connection, a retry. Replaying the same key returns the **first response verbatim** and never re-executes the send.
- **`clientRef`** (body field) — protects against *deliberately sending the same logical message twice*, at the database level, scoped to your `App`. Use your own domain identifier (`otp-login-4471`).

Both can surface as a `409`; `SdkError::is_idempotency_in_flight` and `is_conflict` distinguish them in Rust.

## 6. HTTP status codes

| Status | Meaning                                                                                                                                                              | What to do                                                                                                            |
| ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `200`  | Accepted and persisted.                                                                                                                                              | Record `messageId`.                                                                                                   |
| `401`  | The request was not signed, or its key is unknown, revoked or expired, or its nonce was seen before. The answer is deliberately unsigned and says no more than that. | Do not retry blindly: seal a fresh request, and if it persists ask the operator whether your key is still registered. |
| `403`  | Your client lacks the scope or the policy denied it.                                                                                                                 | Do not retry — the credential needs different scopes.                                                                 |
| `404`  | Unknown id, **or** a row your `App` cannot see.                                                                                                                      | Do not retry. Row-level policy filters rather than erroring.                                                          |
| `409`  | Idempotency replay in flight, a `clientRef` conflict, or an illegal state transition.                                                                                | Distinguish before acting; none of the three is fixed by a blind retry.                                               |
| `429`  | Rate limited.                                                                                                                                                        | Back off; honour `Retry-After`.                                                                                       |
| `5xx`  | Genuine server fault.                                                                                                                                                | Retry with backoff **and** an `Idempotency-Key`.                                                                      |

Rate limiting runs at two points on the API: a per-principal bucket on the API (charged to the verified signer), and a coarser per-source-IP bucket that bounds what a flood of forged identities can reach. An honest client at normal volume sees none of them.

## 7. Receiving webhooks

Delivery is push, not poll. An operator registers a `WebhookEndpoint` for your `App` — **you cannot create one yourself**: it requires an `owner`/`admin`/`developer` human role, and no admin-console screen exists for it yet, so today this is an operator action against the database. Ask for one; the fields that matter to you are the URL, the event types, and whether the recipient MSISDN is masked.

Events (`type` in the envelope):

`message.accepted` · `message.submitted` · `message.delivered` · `message.failed` · `message.expired` · `message.uncertain` · `message.cancelled`

**There is no `message.undelivered`, `message.queued`, `message.routed`, or `message.rejected` event.** If you need to know about a `rejected` message, read it back over REST — a webhook will never tell you.

The envelope:

```json
{
  "id": "c8f2a1b3d4e5f60718293a4b5",
  "type": "message.delivered",
  "occurredAt": "2026-07-28T14:03:11Z",
  "data": {
    "messageId": "c4f2a1b3d4e5f60718293a4b5",
    "appId": "c9c1eb3d4e5f60718293a4b5c",
    "clientRef": "otp-login-4471",
    "to": "+2376xxxxx89",
    "state": "delivered",
    "operator": "orange",
    "segments": 1,
    "deliveredAt": "2026-07-28T14:03:09Z",
    "costXaf": "22.00"
  }
}
```

Headers: `X-Sms-Event`, `X-Sms-Event-Id`, `X-Sms-Timestamp`, `X-Sms-Signature`.

Three obligations on a correct receiver:

1. **Verify the signature.** HMAC-SHA256 over `v1\n{timestamp}\n{eventId}\n{sha256(body)}`, keyed by your endpoint secret. During a rotation window `X-Sms-Signature` carries **two** `v1=` values — accept if *either* verifies.
2. **Dedupe on `X-Sms-Event-Id`.** Delivery is at-least-once. You will eventually see a duplicate.
3. **Ack fast, process after.** Return 2xx immediately; slow handlers trip the endpoint's circuit breaker and get retried.

[`examples/node/webhook-receiver`](../examples/node/webhook-receiver) is a complete, correct implementation of all three, and its signature verification is cross-checked in CI against the Rust implementation and against `openssl` — three independent implementations agreeing, not one asserting.

## 8. Testing failure, not just success

The local stack fakes the carrier with `sms-fake-orange`, which can inject faults on demand — the reason to develop against it rather than a hand-written mock. Restart it with:

- `--fault-mode seeded --seed 3` — a reproducible weighted mix of rejects, 429s, timeouts, and duplicate, out-of-order, or racing delivery receipts.
- `--reject-tokens` — carrier credentials revoked mid-flight.
- `--dlr-delay-ms 30000` — slow delivery, for testing your own polling and timeouts.

Details in [`runbooks/local-development.adoc`](runbooks/local-development.adoc).

## Where to go next

- [`runbooks/local-development.adoc`](runbooks/local-development.adoc) — get the stack running.
- [`examples/README.md`](../examples/README.md) — runnable senders in Rust and Node.
- [`sdks/rust/vsms-sdk-rust`](../sdks/rust/vsms-sdk-rust/README.md) — the Rust SDK.
- [`architecture.md`](architecture.md) — the full design, if you want to know why any of this is shaped the way it is.
