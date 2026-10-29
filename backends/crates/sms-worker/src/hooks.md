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
([`crate::claim::ServedModes::HmacOnly`]). The mode is decided where the rows are
chosen: the claim reads the endpoints first. When every endpoint is one it may take
attempts from (active, circuit closed, `hmac`) it is the keyed claim's own query;
otherwise it asks `webhook_endpoint_due_idx` for each such endpoint's due attempts,
so its cost follows the `hmac` work (per endpoint, up to `budget` rows each) and
never touches a sealed backlog, however large (`claim.md` has the measurements).
Filtering the rows after fetching them would let a sealed backlog, or a deactivated
endpoint's, fill the window and starve the `hmac` rows behind it. A worker that
holds the key claims both modes by the same rule, leaving out only the endpoints
that are deactivated or whose circuit is open.

Nothing about a sealed row is therefore written by a keyless worker: it stays
`pending` (or `failed`, resting out its backoff), its `attempts` unchanged, its
endpoint's breaker untouched, and a worker that has the key delivers it as soon as
it runs.

**One window is left open, and it is closed after the claim.** The claim chose an
`hmac` endpoint, and an operator can switch it to `sealed`
(`switchWebhookDeliveryMode`) before the worker reads the endpoint again to
deliver. The attempt is then `delivering`, counted by the claim, and this worker
cannot send it. `deliver_one` logs it and returns without writing an outcome,
exactly as for an endpoint that vanished: a failure written here would be a lie,
burning an attempt of `maxAttempts` (`dead` for a `maxAttempts` of 1) and
feeding the breaker for a request never made. The lease expires, a keyless
reclaim skips the row (the endpoint is `sealed` now) and a worker that has the key
resumes it without counting a second attempt.

A row the sealer refuses anyway (a signer that fails, or a row written around
the API: the CHECK on a `sealed` row's URL is a pattern, and it is the pattern
**together with `@uri`**, which every write through the API passes, that never
accepts what the sealer refuses, so a URL put into the table with `psql` can
still reach the sealer) is an ordinary [`Outcome::Retryable`] like any other
failed delivery: counted, backed off, able to open the breaker and to go `dead`.

Rows that wait silently are the cost, so [`SealedBacklogWatch`] pays it: a keyless
`run` counts the due sealed attempts of **active** endpoints (a `COUNT(*)`, due
or abandoned by a crashed worker) every five minutes and, if there are any, logs
a `warn` with the number and `--envelope-signing-key-file`. A deactivated
endpoint's attempts are claimed by no worker, key or not, so they are not
waiting for one; an endpoint whose circuit is open is counted, because the
circuit is a pause that ends by itself. `backends/apps/sms-worker` logs one at
start.

# When `hmac_webhooks` is off (VSA-1)

`auth.methods` is deployment configuration (`sms-config`, `auth.md`), and a
deployment that does not permit `hmac_webhooks` (vaam's `[keypair]`) must not
send with a shared secret, whatever rows exist. `WorkerContext::hmac_webhooks`
carries the answer, and it decides **what the worker claims**, not what it does
with a row it already holds: an `hmac` endpoint is excluded from the claim exactly
as a `sealed` endpoint is for a worker without the signing key (VS-8,
[`crate::claim::ServedModes`]). Nothing is written for such an attempt.

| signing key | `hmac_webhooks` | claims                                              |
| ----------- | --------------- | --------------------------------------------------- |
| held        | permitted       | `hmac` and `sealed` ([`ServedModes::Both`])         |
| not held    | permitted       | `hmac` only ([`ServedModes::HmacOnly`])             |
| held        | off             | `sealed` only ([`ServedModes::SealedOnly`])         |
| not held    | off             | nothing ([`ServedModes::Neither`])                  |

```mermaid
stateDiagram-v2
    [*] --> pending: event subscribed (an hmac endpoint still gets its attempts)
    pending --> pending: claims skip it while hmac_webhooks is off (nothing written, attempts unchanged)
    pending --> delivering: claim (attempts + 1), once hmac is permitted again, or the endpoint is sealed
    delivering --> succeeded: 2xx
```

The filter is applied in both claim paths, because it is a condition of the
endpoints the claim reads first (`ServedModes::can_claim`), never a filter over
rows it fetched: when every endpoint is claimable it is the one time-ordered read
of `webhook_due_idx`; when some are not (an `hmac` endpoint with the method off,
among others) it is the per-endpoint fan-out, which never asks about an endpoint
it may not take from. An `hmac` endpoint with 60 or more due attempts, which
nothing purges and a subscriber keeps adding to, therefore cannot fill a
window and starve the `sealed` endpoints behind it, which a "claim, find out,
hand it back" design would have let it do. An expired lease of an `hmac`
endpoint (a crash while the method was permitted) is left alone the same way.

What this means for an attempt, and the reason for each choice:

- **It stays `pending`** (or `failed` and resting, or `delivering` with an expired
  lease: whatever it was). Nothing is written, so nothing costs a lease write, a
  park write or an endpoint read per hour per attempt, and a backlog that grows by
  the event rate costs a row each and no work. An earlier design claimed each
  attempt and parked it as `failed` for an hour; at one event per second the
  re-parks filled a worker within a day and sealed deliveries queued behind them.
- **It is not counted.** `attempts` is incremented at a claim and there is none,
  so `maxAttempts` is untouched: the day the method is permitted again the attempt
  has every try it had, and a month of waiting does not age it into `dead`.
- **It is off the endpoint's breaker.** No request was made, so the endpoint did
  nothing wrong, the reasoning [`Outcome::MalformedPayload`] records for our own
  bugs. `circuitOpenUntil` and `consecutiveFailures` are as they were.
- **It is delivered at the next claim once the method is permitted again**, with no
  hour's wait and no replay. Changing `auth.methods` is a restart of the workers
  (the configuration is read at start), after which the first poll takes it.
- **After `switchWebhookDeliveryMode` to `sealed`, the pending attempts are
  delivered sealed.** The switch rewrites the endpoint row only; the claim reads
  the endpoint's mode afresh, `deliver_one` reads the endpoint again and `send`
  and `build_envelope` choose the request by that mode, and the stored `payload` is
  the same for both. So an attempt created for an `hmac` endpoint is sealed (with
  the sealed envelope's `eventId`, `appId` and `endpointId`) by a worker that holds
  the key, and a worker without it leaves it pending for one that does.
  `an_hmac_attempt_held_back_is_delivered_sealed_after_the_endpoint_is_switched`
  proves it end to end.
- **The sealed attempts beside them are unaffected** and delivered as before.

A worker that serves nothing (no key, method off) is idle by configuration, not by
a fault: `run` logs one `warn` at start saying so, and not a refusal to start,
the way a keyless `hooks` worker is a warning and not an error (VS-8). The
configuration is deployment-wide, so every worker of the deployment has the method
off, and a scale-to-N `hooks` fleet may legitimately have one node with the key
and others without it. Refusing would crash-loop the keyless nodes of that fleet
for a state the keyed ones make useful.

Two five-minute warnings say what is waiting, because a claim loop that skips
rows is silent: [`SealedBacklogWatch`] (a worker with no key; above) and
[`HmacBacklogWatch`], whose `warn` carries `held`, the count of due attempts of
**active** `hmac` endpoints (a `COUNT(*)`, due or abandoned by a crashed worker),
and the ways out. A deactivated endpoint's attempts are claimed by no worker, so
they are not counted. `run` also logs once at start how many `hmac` endpoints exist
(`sms_api::HmacBoot`, the line `serve` logs too). The API side, which refuses to
create, switch to or rotate one, is `sms-api`'s `hmac_gate.md`.

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
