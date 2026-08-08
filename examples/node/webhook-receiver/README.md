# vsms reference webhook receiver

A minimal Express receiver showing what a *correct* handler of vsms delivery
webhooks looks like — the inbound half of a vsms integration. The outbound
half (authenticating and sending) is [#149](https://github.com/vymalo/vsms/issues/149).

## Read this before trusting anything below

**vsms cannot deliver webhooks today.** This is not a hedge, it's the reason
this example exists in its current shape:

- Outbound delivery is entirely unbuilt. The `drain` and `hooks` worker roles
  (`crates/sms-worker`) are stubs that idle; [#38–#42](https://github.com/vymalo/vsms/issues/38)
  are all open.
- `rotateWebhookSecret` is a `not_yet("milestone 3 (webhooks)")` stub in
  `crates/sms-api/src/procedures.rs` — there is no way to even provision a
  live `WebhookEndpoint` secret today.
- **The outbound signature this receiver verifies is [#41](https://github.com/vymalo/vsms/issues/41)
  — not implemented.** It IS specified in real detail, in §4.4 — see below
  for exactly how much of this example is a transcription of that spec
  versus an actual guess. `docs/architecture.md` §4 is explicit that
  request signing was dropped for *inbound* API calls in favour of
  `private_key_jwt`; that decision says nothing about *this*, the outbound
  signature vsms would attach to a webhook it sends you — §4.4 is the
  relevant section instead.

So: **this example cannot be verified end to end against a live vsms, and
nothing here should be read as implying it has been.** What you can verify,
and what running `pnpm start` actually proves, is narrower and still useful:
**this receiver's own logic — idempotent handling, tolerance of
out-of-order delivery, fast-ack-then-process, and signature verification
with rotation support — is correct**, exercised against a local emitter
that stands in for vsms. That is a real, load-bearing distinction; see
"What this does and doesn't prove" below.

## What's settled vs. what's provisional

**Most of this is settled, not provisional.** `docs/architecture.md` §4.4
specifies, literally: the four header names, the exact rotation semantics
(`X-Sms-Signature` can carry two `v1=` values, accept if either verifies),
and the exact signing string
(`v1 \n {timestamp} \n {eventId} \n {sha256(body)}`, keyed by
`WebhookEndpoint.secret`/`.prevSecret`). `src/signature.ts` is a
transcription of that spec, not a guess, for all of the above.

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

Provisional, because #41 hasn't shipped, and narrowly so:

- **The one genuine guess is the MAC algorithm.** §4.4 shows the
  `v1=<hex>` wrapper and the four-line signing string but never names the
  primitive that turns that string into the hex digest. `src/signature.ts`
  implements HMAC-SHA256 as the obvious Stripe-style reading of that shape
  — that one substitution, and only that one, is unverified against
  anything upstream.
- **Separately** (not filling a gap in §4.4 — the doc says nothing about
  this either way, so it's a scope decision by this example, not an
  inferred reading): no bounded freshness/replay window is enforced on
  `X-Sms-Timestamp`. It's folded into the signed bytes, so a *tampered*
  timestamp already fails verification — but a correctly-signed request
  with a stale timestamp still verifies. Demonstrated live, not just
  asserted; see "What it demonstrates, live" below.

When #41 lands for real, `src/signature.ts` — specifically
`computeSignature` and `verifySignature` — is the entire diff this example
should need. Nothing else touches a header or does crypto.

## Layout

```
src/
  index.ts       entry point: starts the receiver, runs the local emitter, prints a summary
  server.ts       the Express app: raw-body capture, fast ack, off-path processing
  signature.ts    THE SEAM — a §4.4 transcription except for one guess (the MAC algorithm); replace/update when #41 lands
  store.ts        in-memory idempotency (primary: X-Sms-Event-Id; secondary: aggregateId+eventType) + out-of-order-tolerant state tracking
  work-queue.ts   a tiny off-request-path queue (what "work off the request path" means, made concrete)
  emitter.ts      local stand-in for vsms; drives #150's required scenarios plus a timestamp-freshness check
  types.ts        the §8.4 envelope shape
```

## Running it

Requires Node with native TypeScript support unflagged (Node ≥23.6; this
repo's own `.nvmrc` pins Node 24, which has it — no `ts-node`/`tsx`
dependency needed).

```bash
cd examples/node/webhook-receiver
pnpm install
pnpm start
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
   not checked for age. See "What's settled vs. what's provisional" above
   for why that's a deliberate scope decision, not an oversight.

Run it yourself and read the log; every line above is something the
process actually printed on this machine, not a description of expected
behaviour.

## What this does and doesn't prove

**Does prove:** this receiver's handling of duplicates, out-of-order
arrival, and signature rejection/rotation is correct, against real HTTP
requests, over a real (if local) network round trip, with a real async
work queue decoupling the response from the "database write" it simulates.

**Does not prove:** that this matches what a real vsms will someday send.
Specifically unverified, and unverifiable until the corresponding work
lands:

- Whether HMAC-SHA256 is really the algorithm #41 ships with — the one
  genuine gap in an otherwise-specified scheme (see above).
- Whether the `X-Sms-Event-Id` value vsms sends is actually the
  `WebhookAttempt`'s `sourceEventId` as this example assumes, or something
  else — §4.4 names the header but the doc's worked JSON example (§8.4)
  doesn't show the header alongside the body, so this is a reasonable but
  unconfirmed reading.
- Whether #41's implementation matches §4.4's design doc exactly once it's
  actually built and tested against a real sender — everything else in
  "What's settled vs. what's provisional" above is specified, but
  "specified" and "implemented-and-verified" are still two different
  things.
- Any latency, retry timing, or backoff behaviour (§8.5 documents 1s, 5s,
  25s, 2m, 10m, 1h, 6h, 24h, eight attempts then `dead` — this example's
  emitter does not attempt to reproduce that schedule; it just proves the
  receiver survives duplicates and reordering, not the exact cadence they
  arrive under).

## Revisit this when M3 (webhooks) lands

- Replace `src/signature.ts`'s MAC algorithm with whatever #41 actually
  ships (the rest of the file should need no change, since it already
  transcribes §4.4's specified header names, rotation semantics, and
  signing string), and delete the narrower "one genuine guess" framing
  throughout this README and the source comments once it's confirmed
  correct.
- Point the emitter (or better, delete it) at a real, running vsms with a
  provisioned `WebhookEndpoint`, once `rotateWebhookSecret` and outbound
  delivery both exist, and re-verify the scenarios above against that
  instead of the local stand-in.
- Confirm the `X-Sms-Event-Id` → `sourceEventId` assumption above, and the
  `data.messageId` → aggregate id assumption in `server.ts`'s
  `extractAggregateId`, against real payloads.
- Decide, deliberately, whether a bounded freshness/replay window on
  `X-Sms-Timestamp` is worth adding on top of the dedupe-based protection
  this receiver already has — #41 may or may not settle vsms's own intent
  on this.
