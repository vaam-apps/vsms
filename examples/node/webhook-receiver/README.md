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
  — not implemented, and not fully specified.** `docs/architecture.md` §4
  is explicit that request signing was dropped for *inbound* API calls in
  favour of `private_key_jwt`; that decision says nothing about *this*,
  the outbound signature vsms would attach to a webhook it sends you.

So: **this example cannot be verified end to end against a live vsms, and
nothing here should be read as implying it has been.** What you can verify,
and what running `pnpm start` actually proves, is narrower and still useful:
**this receiver's own logic — idempotent handling, tolerance of
out-of-order delivery, fast-ack-then-process, and signature verification
with rotation support — is correct**, exercised against a local emitter
that stands in for vsms. That is a real, load-bearing distinction; see
"What this does and doesn't prove" below.

## What's settled vs. what's provisional

Settled, from `docs/architecture.md` (cited inline in the source):

- `WebhookAttempt` has a unique dedupe index on
  `(endpoint_id, aggregate_id, event_type)` — §2.10, §8.3. This receiver
  keys its own idempotency store on that same tuple (`aggregateId` +
  `eventType`), not on `sourceEventId`, because the doc is explicit that
  keying on the event id alone would treat every retry and every
  `Message.updated` touch as a distinct event.
- `WebhookEndpoint` holds both `secret` and `prevSecret` — rotation has a
  designed 24-hour overlap window (§4.4), and a correct receiver accepts
  either.
- §8.5: delivery order is not guaranteed; receivers must tolerate
  `message.delivered` arriving before `message.submitted`.

Provisional, because #41 hasn't shipped:

- **The signature algorithm.** §4.4 documents the header names
  (`X-Sms-Event`, `X-Sms-Event-Id`, `X-Sms-Timestamp`, `X-Sms-Signature:
  v1=<hex>[,v1=<hex>]`) and a signing string
  (`v1\n{timestamp}\n{eventId}\n{sha256(body)}`), but never names the MAC
  algorithm. `src/signature.ts` implements HMAC-SHA256 as the obvious
  Stripe-style reading of that shape — **that choice is a guess, not a
  documented fact**, and is the entire reason this file exists as one
  isolated seam rather than logic folded into the handler.
- Any replay/freshness tolerance on `X-Sms-Timestamp` — §4.4 doesn't specify
  one, so none is enforced beyond the timestamp being part of the signed
  bytes (a tampered timestamp already fails verification).

When #41 lands for real, `src/signature.ts` — specifically
`computeSignature` and `verifySignature` — is the entire diff this example
should need. Nothing else touches a header or does crypto.

## Layout

```
src/
  index.ts       entry point: starts the receiver, runs the local emitter, prints a summary
  server.ts       the Express app: raw-body capture, fast ack, off-path processing
  signature.ts    THE PROVISIONAL SEAM — replace this file when #41 lands
  store.ts        in-memory idempotency + out-of-order-tolerant state tracking
  work-queue.ts   a tiny off-request-path queue (what "work off the request path" means, made concrete)
  emitter.ts      local stand-in for vsms; drives the three required scenarios
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

**A temporary wrinkle, expected to resolve on its own:** this package lives
outside the root `pnpm-workspace.yaml` (which only covers `admin` and
`packages/*`), and the shared `examples/` pnpm workspace (glob `node/*`)
that's meant to pick it up automatically is being added concurrently by the
[#149](https://github.com/vymalo/vsms/issues/149) PR. Until this branch is
rebased on top of that, running `pnpm install` from a plain checkout of
*this* PR alone gets swallowed by the *root* workspace (it walks up and
finds `pnpm-workspace.yaml` there first, sees this directory isn't a
declared member, and silently installs nothing for it). If you're checking
out this PR in isolation before #149 has landed, use:

```bash
pnpm install --ignore-workspace
pnpm start
```

Once both PRs are merged (or once you rebase this branch on top of #149),
plain `pnpm install && pnpm start` from this directory works exactly as
written above — no flag needed, because `examples/pnpm-workspace.yaml`'s
`node/*` glob will pick this package up as the nearest workspace root
before pnpm ever reaches the repo root's.

`pnpm start` boots the receiver on `http://localhost:4790` (override with
`WEBHOOK_RECEIVER_PORT`), then runs the local emitter against it, then
prints a summary and exits. Nothing here binds port 3000, 8080, 8090, or
3100 — those are held by other in-flight work in this repo.

## What it demonstrates, live

The emitter drives four sequences against the receiver over real HTTP
(loopback), in order:

1. **Duplicate delivery.** The exact same `message.delivered` event is
   POSTed twice (an at-least-once sender retrying). First call: processed,
   `HTTP 202`. Second call: recognised via the `(aggregateId, eventType)`
   tuple as already-processed, `HTTP 202` again, but explicitly *not*
   reprocessed — logged as `accepted-duplicate`.
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

- Whether HMAC-SHA256 is really the algorithm #41 ships with.
- Whether the header names, casing, or the exact signing-string layout in
  §4.4 survive unchanged once #41 is implemented and tested against a real
  sender.
- Whether the `X-Sms-Event-Id` value vsms sends is actually the WebhookAttempt's
  `sourceEventId` as this example assumes, or something else.
- Any latency, retry timing, or backoff behaviour (§8.5 documents 1s, 5s,
  25s, 2m, 10m, 1h, 6h, 24h, eight attempts then `dead` — this example's
  emitter does not attempt to reproduce that schedule; it just proves the
  receiver survives duplicates and reordering, not the exact cadence they
  arrive under).

## Revisit this when M3 (webhooks) lands

- Replace `src/signature.ts`'s algorithm/format with whatever #41 actually
  ships, and delete the "provisional" framing throughout this README and
  the source comments.
- Point the emitter (or better, delete it) at a real, running vsms with a
  provisioned `WebhookEndpoint`, once `rotateWebhookSecret` and outbound
  delivery both exist, and re-verify the three scenarios against that
  instead of the local stand-in.
- Confirm the `X-Sms-Event-Id` → `sourceEventId` assumption above, and the
  `data.messageId` → aggregate id assumption in `server.ts`'s
  `extractAggregateId`, against real payloads.
- Consider whether a bounded freshness/replay window on `X-Sms-Timestamp`
  is worth adding, once #41 states whether vsms intends one.
