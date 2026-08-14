# Integrating with vsms

For a developer whose application sends SMS **through** vsms. If you are working *on* vsms itself, start with [`CONTRIBUTING.md`](../CONTRIBUTING.md) and [`architecture.md`](architecture.md) instead; if you want the whole stack running on your laptop, [`runbooks/local-development.md`](runbooks/local-development.md) is the shorter path and this guide is what you read next.

Everything below works against a local stack with no Orange or MTN account, no real handset, and no SMS ever leaving the machine. The credential, the token exchange, the REST calls, and the webhooks are all real — only the carrier is faked.

## 1. Get a credential

vsms has no API keys and no shared secrets. A caller is an **`AppClient`** holding an RSA private key, and authenticates with RFC 7523 `private_key_jwt`: sign a short-lived assertion, exchange it at `POST /token` for a `client_credentials` access token, send that as a Bearer token.

An operator provisions your client and hands you two values:

```bash
sms-gateway provision-client \
  --app-id <an existing, active App.id> \
  --label "billing service — alice" \
  --scope sms:send --scope sms:read \
  --key-out ./client-key.pem
```

It prints `provisioned client: <clientId>` and writes the private key to `--key-out` — once, `0600`, never printed and never stored server-side. If you lose it, you get a new client; there is no recovery.

Give each service (and ideally each developer) its own client against the same `App`. Messages are visible per-`App`, not per-client, so two clients on one `App` can read each other's messages — see [`runbooks/e2e-integration.md`](runbooks/e2e-integration.md) for why that is the intended design.

## 2. Send

Three supported paths. All do the identical thing over real HTTP.

### Rust — the SDK

[`vsms-sdk-rust`](../sdks/rust/vsms-sdk-rust/README.md) owns the whole credential lifecycle, including refresh-on-401. You never touch a JWT.

```rust
let config = PrivateKeyJwtConfig::from_key_path(
    "http://127.0.0.1:8080", "<clientId>", "./client-key.pem", "sms:send sms:read",
)?;
let client = VsmsClient::private_key_jwt("http://127.0.0.1:8080", config)?;

let outcome = client.send_message(SendMessageInput {
    to: "+237677123456".to_owned(),
    body: "Your code is 4471".to_owned(),
    senderId: Some("VYMALO".to_owned()),
    class: None, clientRef: None, scheduledAt: None, validityMinutes: None,
}, Some("retry-safe-key-1")).await?;
```

Runnable: [`examples/rust/sms-send`](../examples/rust/sms-send).

### Node — the example

[`examples/node/sms-send-example`](../examples/node/sms-send-example/src/index.mjs) is one file doing the assertion, the exchange, and the send. Copy it; it mirrors the admin console's own [`frontends/packages/gateway/src/token.ts`](../frontends/packages/gateway/src/token.ts) rather than inventing a second reading of the same spec.

### Any language — raw HTTP

1. Sign a JWT with your private key (RS256), claims `iss`/`sub` = your `clientId`, `aud` = the issuer, plus `jti` and a short `exp`.
2. `POST {issuer}/token`, form-encoded: `grant_type=client_credentials`, `scope=sms:send sms:read`, `client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer`, `client_assertion=<the JWT>`.
3. `POST {issuer}/$procs/sendMessage` with `Authorization: Bearer <token>`.
4. `GET {issuer}/messages/{id}` to read state back.

`sms-gateway routes` prints the full route table and needs no database.

## 3. Request and response

`SendMessageInput` ([`schema.cstack`](../schemas/vsms.cstack)):

| Field | Type | Notes |
|---|---|---|
| `to` | String, required | E.164, `+237…`. Validated and normalised; a non-Cameroon or unallocated number is refused before persistence. |
| `body` | String, required | Encoding is analysed, not assumed — see below. |
| `senderId` | String? | Must be a registered, approved sender id for your `App`. |
| `class` | `otp` \| `transactional` \| `notification` \| `marketing` | Drives routing and quiet-hours rules. |
| `clientRef` | String? | **Your** dedupe key, scoped to the `App`. See §5. |
| `scheduledAt` | DateTime? | Send later. |
| `validityMinutes` | Int? | After this, the message expires rather than delivering. |

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

| Class | Consent record required | Quiet hours |
|---|---|---|
| `otp` | No | None |
| `transactional` | No | None |
| `notification` | **Yes** | None |
| `marketing` | **Yes** | **08:00–20:00 WAT only** |

- **Consent** — sending `notification` or `marketing` to a recipient with no standing `ConsentRecord` for that class fails with a validation error naming the statute. An operator records consent; you cannot self-serve it.
- **Quiet hours** — a `marketing` send is refused outside 08:00–20:00 WAT. The window is checked against the **delivery** time, so `scheduledAt` cannot be used to land one at 22:00. Schedule it inside the window instead.
- **Opt-out** is enforced for every class, and is checked before the message is persisted.

Declaring `otp` on marketing traffic to dodge these checks is not prevented by the code — the declared class is recorded, not verified. It is your legal exposure, not a loophole.

## 4. Message states

What a caller actually observes, in the order they normally occur:

| State | Terminal | Meaning |
|---|---|---|
| `accepted` | | Persisted and validated. Your send succeeded. |
| `queued` | | Claimed by a worker for routing. |
| `routed` | | A provider was selected; submission is in flight. |
| `submitted` | | The carrier accepted it. Awaiting a delivery receipt. |
| `delivered` | ✓ | The carrier confirmed handset delivery. |
| `failed` | ✓ | Permanently failed. `stateReason` says why. |
| `expired` | ✓ | Passed `validityMinutes` without delivering. |
| `rejected` | ✓ | Refused — opt-out, unapproved sender id, invalid recipient. |
| `cancelled` | ✓ | Cancelled via `cancelMessage` before it went out. |
| `uncertain` | | The submit timed out **after** the carrier may have accepted it. |
| `undelivered` | | A retryable carrier failure. |

Two of these need care, and both are deliberate rather than accidental:

- **`uncertain` is never resubmitted.** When a submit's outcome is genuinely unknown, vsms chooses a possibly-lost message over a possibly-duplicated one. For OTP traffic that is the right trade — a user re-requests a code far more cheaply than they get two. A background job eventually resolves the message to a terminal state.
- **`undelivered` currently has no retry driver** ([#122](https://github.com/vymalo/vsms/issues/122)). A message that receives exactly one retryable-failure receipt and no follow-up stays there. If your application needs a retry, implement it on your side against `clientRef` — do not assume vsms will re-send.

## 5. Idempotency: `Idempotency-Key` vs `clientRef`

Two different layers protecting against two different problems. Use both.

- **`Idempotency-Key`** (HTTP header) — protects against *not knowing whether your request landed*: a timeout, a dropped connection, a retry. Replaying the same key returns the **first response verbatim** and never re-executes the send.
- **`clientRef`** (body field) — protects against *deliberately sending the same logical message twice*, at the database level, scoped to your `App`. Use your own domain identifier (`otp-login-4471`).

Both can surface as a `409`; `SdkError::is_idempotency_in_flight` and `is_conflict` distinguish them in Rust.

## 6. HTTP status codes

| Status | Meaning | What to do |
|---|---|---|
| `200` | Accepted and persisted. | Record `messageId`. |
| `401` | Token missing, expired, or invalid. | Re-run the token exchange once. The Rust SDK does this for you. |
| `403` | Your token lacks the scope or the policy denied it. | Do not retry — the credential needs different scopes. |
| `404` | Unknown id, **or** a row your `App` cannot see. | Do not retry. Row-level policy filters rather than erroring. |
| `409` | Idempotency replay in flight, a `clientRef` conflict, or an illegal state transition. | Distinguish before acting; none of the three is fixed by a blind retry. |
| `429` | Rate limited. | Back off; honour `Retry-After`. |
| `5xx` | Genuine server fault. | Retry with backoff **and** an `Idempotency-Key`. |

Rate limiting runs at three points: a per-`client_id` bucket on `/token`, a per-principal bucket on the API, and a coarser per-source-IP bucket that bounds what a flood of forged identities can reach. An honest client at normal volume sees none of them.

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

Details in [`runbooks/local-development.md`](runbooks/local-development.md).

## Where to go next

- [`runbooks/local-development.md`](runbooks/local-development.md) — get the stack running.
- [`examples/README.md`](../examples/README.md) — runnable senders in Rust and Node.
- [`sdks/rust/vsms-sdk-rust`](../sdks/rust/vsms-sdk-rust/README.md) — the Rust SDK.
- [`architecture.md`](architecture.md) — the full design, if you want to know why any of this is shaped the way it is.
