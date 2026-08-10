# vsms reference webhook receiver

A minimal Express receiver showing what a *correct* handler of vsms delivery
webhooks looks like — the inbound half of a vsms integration. The outbound
half (authenticating and sending) is [#149](https://github.com/vymalo/vsms/issues/149).

## Read this before trusting anything below

**vsms cannot deliver webhooks over HTTP yet, but the signature this
receiver verifies is real and confirmed.** Two different claims, worth
keeping separate:

- **Outbound HTTP delivery is still unbuilt.** The `drain` and `hooks`
  worker roles (`crates/sms-worker`) are stubs that idle;
  [#38–#40, #42](https://github.com/vymalo/vsms/issues/38) are open. So
  nothing here has ever received a webhook POST from a live vsms process,
  and this example still can't be verified end to end against one.
- **The signature scheme is [#41](https://github.com/vymalo/vsms/issues/41)
  — implemented, not a guess any more.** `crates/sms-webhook` is the real
  Rust implementation `rotate_webhook_secret`
  (`crates/sms-api/src/procedures.rs`) and the future `hooks` role both
  use. This file's own algorithm — HMAC-SHA256, transcribed from §4.4's
  prose before #41 existed — turned out to match it exactly, and that's no
  longer just asserted: `src/cross-language-vectors.test.ts` loads a
  fixture of signatures computed by a *third*, independent implementation
  (`openssl dgst -sha256 -hmac`) and asserts this file's own
  `verifySignature` agrees with every one. `docs/architecture.md` §4 is
  explicit that request signing was dropped for *inbound* API calls in
  favour of `private_key_jwt`; that decision says nothing about *this*,
  the outbound signature vsms attaches to a webhook it sends you — §4.4 is
  the relevant section instead.

So: **this example still cannot be verified against a live, webhook-sending
vsms — that part hasn't shipped.** What you can verify, and what running
`pnpm start` / `pnpm test` actually proves, is narrower but no longer
carries an asterisk on the crypto: **this receiver's own logic —
idempotent handling, tolerance of out-of-order delivery, fast-ack-then-
process, and signature verification with rotation support, using the
*confirmed* real algorithm — is correct**, exercised against a local
emitter that stands in for vsms until #38–#40 ship. That is a real,
load-bearing distinction; see "What this does and doesn't prove" below.

## What's settled vs. what's still ahead of a real sender

**All of the signature scheme is settled — confirmed, not just specified.**
`docs/architecture.md` §4.4 specifies, literally: the four header names,
the exact rotation semantics (`X-Sms-Signature` can carry two `v1=`
values, accept if either verifies), and the exact signing string
(`v1 \n {timestamp} \n {eventId} \n {sha256(body)}`, keyed by
`WebhookEndpoint.secret`/`.prevSecret`, HMAC-SHA256). `src/signature.ts` is
a transcription of that spec for all of the above, and
`src/cross-language-vectors.test.ts` is what turns "transcription" into
"confirmed against the real implementation."

Settled, cited inline in the source:

- **The dedupe key.** §4.4, in its own words: *"Send `X-Sms-Event-Id` and
  mean it — delivery is at-least-once and receivers need a dedupe key."*
  That's the documented **receiver** contract, and this receiver's primary
  idempotency key. It is a different thing from `WebhookAttempt`'s own
  unique index on `(endpoint_id, aggregate_id, event_type)` (§2.10, §8.3),
  which is vsms's **sender-side** guard against creating duplicate
  `WebhookAttempt` rows — that index says nothing about what a receiver
  should key on once at-least-once HTTP delivery reaches it. This receiver
  also keeps the `(aggregateId, eventType)` tuple as a **secondary,
  defensive** check, strictly stronger than but *not* part of §4.4's
  contract — see `store.ts`'s doc comment for why.
- `WebhookEndpoint` holds both `secret` and `prevSecret` — rotation has a
  designed 24-hour overlap window (§4.4: *"a job clears `prevSecret` after
  24 hours"*), and a correct receiver accepts either while that window is
  open.
- §8.5: delivery order is not guaranteed; receivers must tolerate
  `message.delivered` arriving before `message.submitted`.

**Confirmed by #41, not just settled in prose any more:**

- **The MAC algorithm is HMAC-SHA256.** §4.4 shows the `v1=<hex>` wrapper
  and the four-line signing string but never names the primitive that
  turns that string into the hex digest. `src/signature.ts` implemented
  HMAC-SHA256 as the obvious Stripe-style reading of that shape *before*
  #41 existed — that substitution was this example's one genuine guess.
  `crates/sms-webhook` (#41) is now the real answer, and it agrees:
  `src/cross-language-vectors.test.ts` proves it by checking this file's
  `verifySignature` against fixture signatures neither implementation
  computed (a third, independent `openssl` computation instead) — see
  that test file and `crates/sms-webhook/src/lib.rs`'s own module doc for
  the full cross-language proof.

**Still a deliberate scope decision by this example, not a gap in §4.4 or
in #41** (§4.4 says nothing about this either way, so it isn't an inferred
reading of an ambiguous spec): no bounded freshness/replay window is
enforced on `X-Sms-Timestamp` here. It's folded into the signed bytes, so
a *tampered* timestamp already fails verification — but a correctly-signed
request with a stale timestamp still verifies. Demonstrated live, not just
asserted; see "What it demonstrates, live" below.
`sms_webhook::is_timestamp_fresh` (#41) now ships a composable freshness
check for a caller that wants one — a receiver in this language could call
its own equivalent before or after `verifySignature`; this example simply
doesn't, the same choice it made before #41 existed.

`src/signature.ts` — specifically `computeSignature` and
`verifySignature` — was the entire diff #41 needed here. Nothing else in
this example touches a header or does crypto.

## Layout

```
src/
  index.ts                        entry point: starts the receiver, runs the local emitter, prints a summary
  server.ts                       the Express app: raw-body capture, fast ack, off-path processing
  signature.ts                    THE SEAM — a §4.4 transcription, confirmed correct by #41 (see its own doc comment)
  cross-language-vectors.test.ts  proves signature.ts against fixture signatures crates/sms-webhook (#41) didn't compute
  store.ts                        in-memory idempotency (primary: X-Sms-Event-Id; secondary: aggregateId+eventType) + out-of-order-tolerant state tracking
  work-queue.ts                   a tiny off-request-path queue (what "work off the request path" means, made concrete)
  emitter.ts                      local stand-in for vsms; drives #150's required scenarios plus a timestamp-freshness check
  types.ts                        the §8.4 envelope shape
```

## Running it

Requires Node with native TypeScript support unflagged (Node ≥23.6; this
repo's own `.nvmrc` pins Node 24, which has it — no `ts-node`/`tsx`
dependency needed).

```bash
cd examples/node/webhook-receiver
pnpm install
pnpm test    # the #41 cross-language proof — no server, no network, ~0.1s
pnpm start   # the full local-emitter demo below
```

This package is a member of the `examples/` pnpm workspace (`examples/pnpm-workspace.yaml`,
glob `node/*`), which is a *separate* workspace from the repo root's — so installing here
never touches `admin/`'s lockfile. `pnpm install` from this directory resolves
`examples/` as its nearest workspace root and works with no flags.

One trap worth knowing if you ever add an example *outside* that glob: pnpm walks up to
the nearest `pnpm-workspace.yaml`, and if the directory is not a declared member it
**installs nothing and exits successfully** — no error, no warning. This package hit
exactly that before the `node/*` glob existed.

`pnpm start` boots the receiver on `http://localhost:4790` (override with
`WEBHOOK_RECEIVER_PORT`), then runs the local emitter against it, then
prints a summary and exits. Nothing here binds port 3000, 8080, 8090, or
3100 — those are held by other in-flight work in this repo.

## What it demonstrates, live

The emitter drives these sequences against the receiver over real HTTP
(loopback), in order:

1. **Duplicate delivery.** The exact same `message.delivered` event
   (same `X-Sms-Event-Id`) is POSTed twice (an at-least-once sender
   retrying). First call: processed, `HTTP 202`. Second call: recognised
   as a duplicate by §4.4's own documented key — `X-Sms-Event-Id` — `HTTP
   202` again, but explicitly *not* reprocessed, logged
   `accepted-duplicate` and tagged "primary contract per §4.4".
2. **Out-of-order arrival.** `message.delivered` for a message is POSTed
   *before* `message.submitted` for the same message. The receiver applies
   `delivered` (a higher-precedence, more-settled state) first; when the
   late `submitted` arrives, it's accepted (`HTTP 202` — it's a legitimate
   event, not an error) but logged as `accepted-out-of-order-ignored`,
   because applying it would regress the tracked state backwards. The
   aggregate's displayed state stays `message.delivered`, not corrupted
   back to `message.submitted`.
3. **Bonus — rotation overlap.** A third event for the same message,
   signed with `prevSecret` instead of `secret`, is accepted normally —
   proving the receiver's rotation tolerance live, not just asserting it in
   a comment.
4. **Bad signature.** An event signed with neither known secret is POSTed.
   Rejected with `HTTP 401` *before* the body is even parsed as JSON — a
   receiver should not act on bytes it can't attribute to vsms.
5. **Additional check, not one of #150's four required cases — timestamp
   freshness.** An event is signed correctly, but for a timestamp 30 days
   in the past (a genuine signature over stale material, not a forged
   one). It's accepted, `HTTP 202` — proving live, rather than only
   asserting in `signature.ts`'s doc comment, that `X-Sms-Timestamp` is
   not checked for age. See "What's settled vs. what's still ahead of a
   real sender" above for why that's a deliberate scope decision, not an
   oversight.

Run it yourself and read the log; every line above is something the
process actually printed on this machine, not a description of expected
behaviour.

## What this does and doesn't prove

**Does prove:** this receiver's handling of duplicates, out-of-order
arrival, and signature rejection/rotation is correct, against real HTTP
requests, over a real (if local) network round trip, with a real async
work queue decoupling the response from the "database write" it simulates
— **and**, separately (`cross-language-vectors.test.ts`, no HTTP involved),
that `verifySignature`'s algorithm itself is HMAC-SHA256 over exactly
§4.4's canonical string, agreeing with `crates/sms-webhook` (#41) and with
a third, independent computation neither of them produced.

**Does not prove:** that this matches what a real, webhook-*sending* vsms
will someday do end to end. Specifically unverified, and unverifiable
until the corresponding work lands:

- Whether the `X-Sms-Event-Id` value vsms sends is actually the
  `WebhookAttempt`'s `sourceEventId` as this example assumes, or something
  else — §4.4 names the header but the doc's worked JSON example (§8.4)
  doesn't show the header alongside the body, so this is a reasonable but
  unconfirmed reading.
- Whether the `hooks` role (#40), once built, actually calls
  `crates/sms-webhook` the way this README assumes it will, and attaches
  the four headers exactly as documented — #41 ships the signing library
  and `rotateWebhookSecret`, not the HTTP delivery code that would call it.
- Any latency, retry timing, or backoff behaviour (§8.5 documents 1s, 5s,
  25s, 2m, 10m, 1h, 6h, 24h, eight attempts then `dead` — this example's
  emitter does not attempt to reproduce that schedule; it just proves the
  receiver survives duplicates and reordering, not the exact cadence they
  arrive under).

## Revisit this when M3 (webhooks) fully lands

- Point the emitter (or better, delete it) at a real, running vsms with a
  provisioned `WebhookEndpoint`, once the `hooks` role (#40) exists and
  actually sends HTTP requests, and re-verify the scenarios above against
  that instead of the local stand-in.
- Confirm the `X-Sms-Event-Id` → `sourceEventId` assumption above, and the
  `data.messageId` → aggregate id assumption in `server.ts`'s
  `extractAggregateId`, against real payloads.
- Decide, deliberately, whether a bounded freshness/replay window on
  `X-Sms-Timestamp` is worth adding on top of the dedupe-based protection
  this receiver already has — `sms_webhook::is_timestamp_fresh` (#41) is
  available for it, but nothing requires a receiver to call it, and this
  example still doesn't.
