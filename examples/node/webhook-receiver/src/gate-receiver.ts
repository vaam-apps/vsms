/**
 * #44's own entry point — the M3 gate's "a sample Node receiver verifies
 * the signature with an independent implementation" assertion, against a
 * *real*, running `sms-worker --roles hooks` process, not the local
 * `emitter.ts` stand-in `index.ts` drives.
 *
 * `index.ts` (this package's own demo) exists to prove this receiver's
 * *own* logic in isolation, since — as its own README says — "vsms cannot
 * deliver webhooks over HTTP yet" was true when it was written. #40 changed
 * that: `hooks` now really does sign and POST. This file is the receiver
 * half of proving that live delivery path end to end, using the exact same
 * `createReceiver`/`verifySignature` code `index.ts` already exercises
 * against the local emitter — no second, drifting implementation.
 *
 * Unlike `index.ts`, this does NOT run `emitter.ts` — nothing here
 * fabricates a request. Every request this process ever sees comes from a
 * real `hooks` role over a real loopback HTTP connection.
 *
 * The one addition over `server.ts`'s own contract: a `GET
 * /__test__/results` diagnostic route, mounted on the same Express app
 * (`server.ts`'s `Receiver.app`), so the Rust test driving this process can
 * poll for what the receiver actually verified over HTTP rather than
 * scraping stdout — a real, independent observation channel, not a shared
 * memory shortcut (this process and the Rust test are separate OS
 * processes either way). This route is test-only scaffolding; it changes
 * nothing about `/webhooks/vsms`'s own signature-verification contract.
 *
 * ```bash
 * WEBHOOK_RECEIVER_PORT=4790 \
 * WEBHOOK_RECEIVER_SECRET=whsec_... \
 * WEBHOOK_RECEIVER_PREV_SECRET=whsec_... \
 *   node src/gate-receiver.ts
 * ```
 */
import { createReceiver } from "./server.ts";
import type { ProcessedResult } from "./types.ts";

const PORT = Number(process.env.WEBHOOK_RECEIVER_PORT ?? 4790);
const SECRET = process.env.WEBHOOK_RECEIVER_SECRET ?? "demo-current-secret-do-not-use-in-prod";
const PREV_SECRET = process.env.WEBHOOK_RECEIVER_PREV_SECRET;

const results: ProcessedResult[] = [];

const receiver = createReceiver({
  port: PORT,
  secret: SECRET,
  prevSecret: PREV_SECRET,
  onProcessed: (result) => {
    results.push(result);
    console.log(`[gate-receiver] ${JSON.stringify(result)}`);
  },
});

// Test-only: lets the driving Rust process observe, over real HTTP, what
// this independent Node implementation actually verified — without
// touching `/webhooks/vsms`'s own request handling at all.
receiver.app.get("/__test__/results", (_req, res) => {
  res.status(200).json({ results });
});

await receiver.listen();
console.log(`[gate-receiver] listening on http://127.0.0.1:${PORT}`);
console.log("[gate-receiver] waiting for a real vsms hooks role to POST /webhooks/vsms ...");
