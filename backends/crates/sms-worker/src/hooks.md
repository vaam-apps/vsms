`Role::Hooks`'s real body — #40. Claims due `WebhookAttempt` rows via
[`crate::claim::claim_batch`], delivers each in its endpoint's mode with
`sms-webhook` (`sealed`: a `COSE_Sign1` request, VS-8; `hmac`: the original
`X-Sms-Signature`, #41, kept by VS-8R), and drives `pending`/`failed -> delivering -> succeeded|failed|dead`
per §8.5. `claim.rs`'s own `impl Claimable for WebhookAttempt` owns the
claim half (endpoint-health filtering, the crash-reclaim same-state
write); this module owns everything after a row reaches `delivering`.

# The missing transition table, and the position this PR takes on it

`AttemptState` shipped with #38/#39, but no `attempt_state_transitions`
table or trigger existed — nothing had written `WebhookAttempt.state` yet,
so R2 ("proposed by Rust, decided by Postgres") had nothing to decide
against. #38/#39's own PR left this exactly as open as it found it,
flagging it as the next role's problem. This PR resolves it in favour of
the same discipline `messages`/`jobs` already get — `attempt_state_
transitions` + `attempts_guard_transition` (§2.10, `0002_bootstrap`) —
rather than arguing webhook attempts are somehow exempt from R2. They
aren't: an illegal edge here (a bug in this file, a future admin-console
replay feature, a stray `psql` session) deserves the same SQLSTATE `SM001`
→ `409 Conflict` backstop every other state machine in this system gets,
not a silent write or an opaque `500`.

# What `WebhookAttempt.payload` contains, and what actually gets sent

`payload` (written by `backends/crates/sms-api/src/webhooks.rs`'s subscribers) is
the §8.4 `data` object *only* — not the outer envelope. This module
builds the envelope at delivery time ([`build_envelope`]): `id` from
`WebhookAttempt.id` (the only value that has existed since the row was
created — see `webhooks.rs`'s own doc for why the envelope's `id` can't
be anything else), `eventId` from `sourceEventId` (it used to ride only in the
`X-Sms-Event-Id` header, which the HMAC signed; headers are no part of a seal,
so the id is in the sealed body now), `appId` and `endpointId` from the endpoint
the attempt belongs to (every app's deliveries are signed by the one gateway
key, so a receiver tells a delivery made *for it* from one merely addressed to
its audience by these two, which the seal covers), `type` from `eventType`, `data` from `payload` parsed
back into a JSON value (never re-wrapped as a string — nesting an
already-JSON string inside a JSON string is exactly the kind of "close
but not the contract" bug a receiver's own parser would silently choke
on), and `occurredAt` — see [`build_envelope`]'s own doc for why this is
a documented approximation, not the original event's timestamp.

**What gets sealed is exactly what gets sent — never a second, later
re-serialization of the same logical value.** [`build_envelope`] returns
one `String`; its bytes are what [`sms_webhook::Sealer::seal`] puts inside the
`COSE_Sign1` message, which is what `reqwest` puts on the wire. Sealing
anything else — the parsed `serde_json::Value` re-serialized a second
time by a different call site, say — would be the exact silent bug this
module's own doc warns about: `serde_json::Value`'s map type does not
preserve key order or exact whitespace, so a second serialization is not
guaranteed to produce byte-identical output to the first.

# The mode is the endpoint's, per row (VS-8R)

`WebhookEndpoint.deliveryMode` picks how one attempt is built, and nothing else
about the loop differs: the claim, the status handling (2xx delivered, 410
deactivates, anything else retries with the §8.5 backoff and counts toward the
breaker), `write_outcome` and the 10 s timeout are one path for both. The redirect
policy is the one thing that is not: `sealed` follows none, `hmac` keeps reqwest's
default (`HttpClients`).

|                                                   | `sealed`                                                                   | `hmac`                                                                                                  |
| ------------------------------------------------- | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| Body ([`build_envelope`])                         | `{id, eventId, type, occurredAt, appId, endpointId, data}`                 | `{id, type, occurredAt, data}`, what its receivers verify today                                         |
| Authentication                                    | `Sealer::seal(audience, url, body)`, fresh per attempt                     | `sign_header([secret, prevSecret?], timestamp, eventId, body)`, the current secret first                |
| Headers                                           | the seal's `Content-Type` and `Cratestack-*`                               | `X-Sms-Event`, `X-Sms-Event-Id`, `X-Sms-Timestamp`, `X-Sms-Signature`, `Content-Type: application/json` |
| Needs                                             | `WorkerContext::webhook_sealer` (the envelope signing key) and an audience | the endpoint's `secret`                                                                                 |

# A worker without the signing key claims no sealed attempt

A worker with no signing key still starts and still delivers every `hmac`
endpoint, and it does not **claim** a `sealed` attempt at all
([`crate::claim::ServedModes::HmacOnly`]). The filter is in the claim's own query,
a correlated subquery on the endpoint's `deliveryMode` for the fresh claim and the
expired-lease reclaim alike, placed ahead of the over-fetch window: filtering the
rows after fetching them would let a sealed backlog fill the window and starve the
`hmac` rows behind it. A worker that holds the key claims both modes.

Nothing about a sealed row is therefore written by a keyless worker: it stays
`pending` (or `failed`, resting out its backoff), its `attempts` unchanged, its
endpoint's breaker untouched, and a worker that has the key delivers it as soon as
it runs. A row the sealer refuses anyway (a signer that fails; the database already
refuses an audience or URL it cannot seal) is an ordinary [`Outcome::Retryable`]
like any other failed delivery: counted, backed off, able to open the breaker and
to go `dead`.

Rows that wait silently are the cost, so [`SealedBacklogWatch`] pays it: a keyless
`run` counts the due sealed attempts (a `COUNT(*)`, due or abandoned by a crashed
worker) every five minutes and, if there are any, logs a `warn` with the number
and `--envelope-signing-key-file`. `backends/apps/sms-worker` logs one at start.

# Redirects

`build_http_client` builds an `HttpClients` pair. The `sealed` client follows
none: a `307` would re-send bytes sealed for one audience to another host, and is
an ordinary failed attempt. The `hmac` client keeps reqwest's default policy
(up to ten hops, a `307`/`308` re-sending the signed body), exactly as the
pre-sealed worker did, because other apps' receivers sit behind redirects and the
HMAC scheme is kept unchanged for them. The SSRF-shaped caveat is the old one:
whoever controls an `hmac` receiver's redirects controls where its deliveries
go. Both carry the same 10 s timeout. `hooks_live_postgres.rs` proves each.

# Sealing, per attempt (VS-8)

```mermaid
sequenceDiagram
    participant C as claim_batch
    participant H as hooks::deliver_one
    participant S as Sealer
    participant R as Receiver
    C->>H: attempt (delivering, attempts = n)
    H->>H: fetch the endpoint (audience, url), build the envelope
    H->>S: seal(audience, url, envelope)
    S-->>H: COSE_Sign1 with a fresh cti and iat
    H->>R: POST url, sealed bytes, no redirect followed
    R-->>H: status
    H->>H: write_outcome (succeeded / failed + backoff / dead / 410)
```

A retry runs this whole path again: **every attempt is sealed afresh**, so a
retry after the receiver's 300 s skew still opens and a re-sent nonce is never
what a receiver sees. The endpoint's `audience` is what the seal binds as the
recipient; the route is the path of its `url` (`sms_webhook::delivery_route`).
A delivery that cannot be sealed never reaches the network and is an ordinary
[`Outcome::Retryable`]. The signing key is `WorkerContext::webhook_sealer`, loaded
from `--envelope-signing-key-file`; with none, the worker claims no `sealed`
attempt (above) and `hmac` endpoints are unaffected. The receiver's answer is judged by status alone and
never opened (`sms-webhook`'s `lib.md`, "The answer"). A sealed delivery follows
no redirect.

# `maskRecipient` — enforced upstream, not re-derived here

§4.4/§8.4: an endpoint configured for masked recipients must never see a
plaintext MSISDN. That masking happens once, at insert time, in
`backends/crates/sms-api/src/webhooks.rs::message_payload` — `payload`'s `to`
field is already whatever the matched endpoint's `maskRecipient` called
for by the time this module ever reads the row. This module's own
correctness obligation is narrower but just as real: never reconstruct
or enrich that value from anything else this crate can reach (the
`Message` row, if this crate ever gained a reason to read one here) —
[`build_envelope`] parses `payload` into a `data` object and embeds it
verbatim, touching no field of it individually, so there is no code path
by which a plaintext MSISDN could re-enter the outbound body even by
accident. `hooks_live_postgres.rs`'s `mask_recipient_payload_is_forwarded_
verbatim_never_reconstructed` test proves this against a real HTTP
capture, not just by reading this paragraph.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
