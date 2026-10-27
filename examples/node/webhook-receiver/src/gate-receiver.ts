/**
 * #44's own entry point — the M3 gate's "a sample Node receiver opens the
 * delivery with an independent implementation" assertion, against a *real*,
 * running `sms-worker --roles hooks` process.
 *
 * `index.ts` (this package's own demo) exists to show this receiver's *own*
 * logic against the committed vector. This file is the receiver half of
 * proving the live delivery path end to end, using the exact same
 * `createReceiver` code, with the Node SDK's `WebhookOpener` (cratestack's
 * envelope, compiled to WebAssembly) as the verifier: an implementation that
 * shares no code with the worker's sealer in `sms-webhook`.
 *
 * Nothing here fabricates a request. Every request this process ever sees
 * comes from a real `hooks` role over a real loopback HTTP connection.
 *
 * The one addition over `server.ts`'s own contract: a `GET /__test__/results`
 * diagnostic route, mounted on the same Express app, so the Rust test driving
 * this process can poll for what the receiver actually did over HTTP rather
 * than scraping stdout. It is test-only scaffolding; it changes nothing about
 * `/webhooks/vsms`'s own verification.
 *
 * ```bash
 * WEBHOOK_RECEIVER_PORT=4790 \
 * WEBHOOK_RECEIVER_SERVER_CURRENT=<base64url> \
 * WEBHOOK_RECEIVER_SERVER_NEXT=<base64url> \
 * WEBHOOK_RECEIVER_AUDIENCE=<audience of the endpoint> \
 * WEBHOOK_RECEIVER_APP_ID=<id of the app the endpoint belongs to> \
 *   node src/gate-receiver.ts
 * ```
 */
import { openerFromEnv, ROUTE } from "./config.ts";
import { createReceiver } from "./server.ts";
import type { ProcessedResult } from "./types.ts";

const PORT = Number(process.env.WEBHOOK_RECEIVER_PORT ?? 4790);

const results: ProcessedResult[] = [];

const receiver = createReceiver({
  port: PORT,
  opener: openerFromEnv(),
  onProcessed: (result) => {
    results.push(result);
    console.log(`[gate-receiver] ${JSON.stringify(result)}`);
  },
});

receiver.app.get("/__test__/results", (_req, res) => {
  res.status(200).json({ results });
});

await receiver.listen();
console.log(`[gate-receiver] listening on http://127.0.0.1:${PORT}`);
console.log(`[gate-receiver] waiting for a real vsms hooks role to POST ${ROUTE} ...`);
