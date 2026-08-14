import type { Server } from "node:http";
import { runEmitterScenarios } from "./emitter.ts";
import { createReceiver } from "./server.ts";
import type { ProcessedResult } from "./types.ts";

const PORT = Number(process.env.WEBHOOK_RECEIVER_PORT ?? 4790);
// Demo-only secrets. A real deployment reads WebhookEndpoint.secret /
// .prevSecret (§2.10) from wherever it's configured — never hardcoded.
const SECRET = process.env.WEBHOOK_RECEIVER_SECRET ?? "demo-current-secret-do-not-use-in-prod";
const PREV_SECRET =
  process.env.WEBHOOK_RECEIVER_PREV_SECRET ?? "demo-previous-secret-do-not-use-in-prod";

console.log("============================================================");
console.log(" vsms reference webhook receiver (examples/node/webhook-receiver)");
console.log("============================================================");
console.log(" vsms cannot deliver webhooks over HTTP yet — the drain/hooks");
console.log(" worker roles (#38-#40, #42) are unbuilt. The signature this");
console.log(" receiver verifies (#41) IS implemented and confirmed against");
console.log(" backends/crates/sms-webhook (see `pnpm test`). See README.md and");
console.log(" src/signature.ts for exactly what that does and doesn't prove.");
console.log("------------------------------------------------------------");
console.log(` listening on   : http://localhost:${PORT}`);
console.log(" webhook route  : POST /webhooks/vsms");
console.log(" health route   : GET  /healthz");
console.log("============================================================\n");

const results: ProcessedResult[] = [];

const receiver = createReceiver({
  port: PORT,
  secret: SECRET,
  prevSecret: PREV_SECRET,
  onProcessed: (result) => {
    results.push(result);
    console.log(`[receiver] ${result.status.padEnd(28)} ${result.detail}`);
  },
});

let server: Server | undefined;

try {
  server = await receiver.listen();

  await runEmitterScenarios({
    baseUrl: `http://localhost:${PORT}`,
    secret: SECRET,
    prevSecret: PREV_SECRET,
  });

  // Give the last queued task a moment to finish before summarising.
  await new Promise((resolve) => setTimeout(resolve, 200));

  console.log("\n=== summary ===");
  const counts = results.reduce<Record<string, number>>((acc, r) => {
    acc[r.status] = (acc[r.status] ?? 0) + 1;
    return acc;
  }, {});
  for (const [status, count] of Object.entries(counts)) {
    console.log(`  ${status.padEnd(28)} x${count}`);
  }

  console.log("\n=== tracked aggregate state (post-demo) ===");
  for (const messageId of new Set(
    results.map((r) => r.aggregateId).filter((id): id is string => Boolean(id)),
  )) {
    const state = receiver.store.get(messageId);
    console.log(
      `  ${messageId} -> ${state ? state.eventType : "(no state recorded — signature was rejected)"}`,
    );
  }

  console.log(
    "\nDemo complete. This proves the receiver's own logic (idempotency, " +
      "ordering, signature verification) works as designed against this " +
      "local emitter. It does NOT prove any of this matches a real vsms " +
      "delivery, because vsms cannot send one yet — see README.md.",
  );
} finally {
  server?.close();
}
