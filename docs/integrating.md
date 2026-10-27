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

The operator console signs in the browser (VS-5b, `docs/design/console-pairing.md`), and `examples/node/demo-app` signs through the Node SDK. There is no compatibility mode to fall back on, and no token endpoint (the OpenID Provider was deleted in VS-4).

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

Delivery is push, not poll. An operator registers a `WebhookEndpoint` for your `App` in the admin console — **you cannot create one yourself**: it requires an `owner`/`admin`/`developer` human role. Ask for one; the fields that matter to you are the URL, **how deliveries are authenticated** (the delivery mode, below), the event types, and whether the recipient MSISDN is masked. An `hmac` endpoint's URL is any URL. A **`sealed`** endpoint's URL must be one a delivery can be sealed for, because the seal binds its path as the route: `http` or `https`, no credentials, no query, no fragment, no `{`, `}` or `%` in the path, and an ASCII host (an internationalised name is written in punycode).

### Events

The events a delivery carries, in either mode (`type` in the envelope):

`message.accepted` · `message.submitted` · `message.delivered` · `message.failed` · `message.expired` · `message.uncertain` · `message.cancelled`

**There is no `message.undelivered`, `message.queued`, `message.routed`, or `message.rejected` event.** If you need to know about a `rejected` message, read it back over REST — a webhook will never tell you.

### Choosing a delivery mode

An endpoint is delivered to in one of two modes, chosen when it is created. Both are always available.

|                                 | `sealed` (recommended)                                                                      | `hmac` (legacy)                                                               |
| ------------------------------- | ------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| How a delivery is authenticated | A request signed with the gateway's own key                                                 | `X-Sms-Signature`, HMAC-SHA256 under a secret shared with you                 |
| What you hold                   | The gateway's two **public** keys, an audience, your endpoint id and app id: nothing secret | The endpoint's secret (and the previous one during a rotation)                |
| What you run                    | The Rust or Node SDK's `WebhookOpener`, or a cratestack `EnvelopeLayer`                     | Your own HMAC check (the scheme is below; no SDK ships one)                   |
| Rotation                        | None of your own: the gateway key's rotation is the one every receiver already follows      | `rotateWebhookSecret`; the old secret keeps verifying until the next rotation |
| Existing endpoints              | New ones                                                                                    | **Every endpoint that existed before the sealed mode is `hmac`, unchanged**   |

**Use `sealed` for anything new**, and for any receiver that can run an opener: you hold nothing secret, so a leaked database or backup cannot forge a delivery to you. Keep `hmac` for a receiver that already verifies `X-Sms-Signature` and has not moved yet; `hmac` is a legacy mode that a deployment will later be able to switch off, so plan the move. Moving an endpoint is the `switchWebhookDeliveryMode` procedure, called by an operator (`owner`, `admin` or `developer`, with `webhook:manage`): `POST /$procs/switchWebhookDeliveryMode` with `{"args": {"endpointId": "<id>", "mode": "sealed" | "hmac", "audience": "<your audience>"}}`. It validates first, answers `422` with the reason where a raw `PATCH` would be refused by the database (as a `500`), and does the bookkeeping:

- to `sealed`: `audience` is required (and must be free: a taken one is `409`), the endpoint's URL must be one a delivery can be sealed for (an `hmac` endpoint registered with a query string cannot move until its URL changes), and the secret, previous secret and rotation stamp are cleared.
- to `hmac`: send no `audience` (it is refused and the endpoint's own is released); a new secret is minted server-side and returned in the answer, and your receiver must be given it, because a sealed endpoint held none to reuse.

Switching to the mode the endpoint is already in is refused. Creating an endpoint takes the same fields: `deliveryMode` is required, a `sealed` one names its `audience` and an `hmac` one carries its `secret` (at least 32 characters; the console generates `whsec_` and 64 random hex characters, and so should a client; `rotateWebhookSecret` and `switchWebhookDeliveryMode` mint every later one). An `hmac` endpoint's secret is returned to the operator roles that can write the endpoint, and is redacted from the audit history. (A raw `PATCH` of `deliveryMode` is still accepted by the generated route for an operator who moves every column in step, with the endpoint's `If-Match`; one that does not breaks a CHECK of the database and is answered `500`, not `422`. Use the procedure.)

### Sealed mode

Choose an audience **only you can have**: your own origin (`https://hooks.example.com`) is the natural one. The gateway gives each name to one endpoint, so a generic name may be taken, and the operator creating your endpoint cannot be asked to guess. It cannot be `vsms`.

**A sealed delivery is a sealed request, not a signed one.** The body is a `COSE_Sign1` message signed with the gateway's own key — the one that signs its answers — so there is no webhook secret to store, no signature header to check and nothing to rotate. You pin the two public keys you already pin for the SDK (`sms-gateway server-keys`) and open the body. **One key signs every app's deliveries**, so the signature and the audience tell you only that vsms sent this to your name; the envelope also carries the `endpointId` and `appId` it was made for, inside the signed body, and you compare them with your own (obligation 1). The `endpointId` is the binding: a receiver serves exactly one endpoint.

The envelope of a sealed delivery (inside the sealed body):

```json
{
  "id": "c8f2a1b3d4e5f60718293a4b5",
  "eventId": "7d1c3f0e-2b9a-4c5e-9a61-0f3b8e2d4a17",
  "type": "message.delivered",
  "occurredAt": "2026-07-28T14:03:11Z",
  "appId": "c9c1eb3d4e5f60718293a4b5c",
  "endpointId": "cd7e0f1a2b3c4d5e6f7a8b9c0",
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

`id` is the dedupe key. `eventId` is the id of the event that produced it. `endpointId` is your endpoint's id and `appId` your `App`'s: both are shown on the endpoint's screen in the console, and the endpoint id is also in the answer when the endpoint is created. A sealed delivery sends none of the `X-Sms-*` headers of the HMAC scheme: a header is no part of a seal, so nothing outside the sealed body is meant to be believed.

Three obligations on a correct receiver:

1. **Open the body.** With the Node SDK, `new WebhookOpener({ serverKeys, audience, route, appId, endpointId }).open(rawBody)` (`rawBody` is the request's bytes, a `Buffer`; not text a framework decoded); with the Rust SDK, `WebhookOpener::new(&server_keys, audience, route, app_id, endpoint_id)?.open(&body).await`. `audience` is the name on your endpoint, `route` is the **whole path** of its URL (one literal route, such as `/webhooks/vsms`, with no query; nothing in front of you may rewrite it: an ingress that strips a prefix makes every delivery a `401`), `app_id` is your `App`'s id and `endpoint_id` your endpoint's. There is no opener without both. **`endpointId` is the binding**: a receiver serves exactly one endpoint, whose id it learns when the endpoint is created, and it is the one signed value nobody but the gateway chooses; whoever may create endpoints can register one under your `app_id` and, whenever your audience name is free, give it that name too. `appId` additionally catches an endpoint registered under another app, and the unique audience is defence in depth, not the binding. Anything that does not open — a forgery, a body sealed by a key you did not pin, a delivery made for another app or endpoint, a replayed body, a timestamp more than 300 seconds off — is **one** answer: reply `401`. A body addressed to a different audience or route does not verify at all, for the same answer. Behind several replicas, give the Rust opener a nonce store they share (the default is per process; the Node opener's cannot be replaced), or lean on obligation 2.
2. **Dedupe on `id`.** Delivery is at-least-once. A retry, and an operator's replay, arrive as a *new* sealed request (new nonce, new timestamp) with the same `id`; the nonce stops a captured request being replayed, it is not your dedupe key. You will eventually see a duplicate.
3. **Ack fast, process after.** Return 2xx immediately; slow handlers trip the endpoint's circuit breaker and get retried. vsms judges a delivery by the status alone and never opens your answer.

If you run your own cratestack `EnvelopeLayer` instead of an SDK, declare the route as `POST <path>` with the contract digest `c61394a50e9f02ece411674fb3f398ae323e685b7a14f9c7b85538aa33a30423`, `application/json` payloads, your audience, and the two pinned keys, and **check `appId` in your handler** (the layer cannot: it opens the seal and knows nothing of apps); `backends/crates/sms-webhook/src/lib.md` has the exact snippet and `backends/crates/sms-webhook/tests/sealed_delivery.rs` runs it.

[`examples/node/webhook-receiver`](../examples/node/webhook-receiver) is a complete, correct implementation of all three for a sealed endpoint. Its opener is checked in CI against a delivery the gateway's own sealer produced, in two languages, and against a live `sms-worker` in a separate process.

### HMAC mode

An `hmac` endpoint's deliveries are what they have always been. The body is the four-field envelope (`id`, `type`, `occurredAt`, `data`: no `eventId`, `appId` or `endpointId`), and four headers carry the rest:

```text
X-Sms-Event:      message.delivered
X-Sms-Event-Id:   c8f2a1...            (the dedupe key, signed)
X-Sms-Timestamp:  1753699200
X-Sms-Signature:  v1=<hex>[,v1=<hex>]  (the second value only during a rotation)
```

**Redirects.** An `hmac` delivery follows redirects as it always has (up to ten, a `307` or `308` re-sending the signed body), so a receiver behind a redirect keeps working; the signed body and signature travel to wherever the redirect points, so keep the redirects of a receiver you register under your control. A `sealed` delivery never follows one: a redirect is a failed attempt, and the URL is fixed on the endpoint.

**Nothing about an `hmac` delivery changed when the sealed mode arrived**, whatever the endpoint's secret is: the bytes, the headers and the signature are what a receiver written against the first version verifies. That includes an old endpoint whose secret is short or empty, which is still signed with it; vsms logs a warning naming such an endpoint, and the fix is `rotateWebhookSecret` (twice when the secret is empty), after which the new secret has to reach your receiver.

Three obligations on a correct `hmac` receiver:

1. **Verify the signature.** HMAC-SHA256 over `v1\n{timestamp}\n{eventId}\n{sha256(body)}`, with the **raw request bytes** in the hash and the endpoint's secret as the key, compared in constant time. During a rotation window `X-Sms-Signature` carries **two** `v1=` values, current secret first: accept if *either* verifies against *any* secret you hold. There is no freshness window in the scheme (the timestamp is signed, so a tampered one fails, but a captured request verifies at any later time); dedupe is what stops a replay being acted on twice.
2. **Dedupe on `X-Sms-Event-Id`.** Delivery is at-least-once. You will eventually see a duplicate.
3. **Ack fast, process after.** Return 2xx immediately; slow handlers trip the endpoint's circuit breaker and get retried.

The Rust and Node SDKs open sealed deliveries only. [`examples/node/webhook-receiver`](../examples/node/webhook-receiver) serves either mode (`WEBHOOK_RECEIVER_MODE=hmac`), and its `src/signature.ts` is a from-scratch verifier whose results are cross-checked in CI against the Rust implementation and against `openssl` (`backends/crates/sms-webhook/tests/fixtures/cross_language_vectors.json`), three independent implementations agreeing, not one asserting.

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
