import { randomUUID } from "node:crypto";
import { setTimeout as sleep } from "node:timers/promises";
import { computeSignature } from "./signature.ts";
import type { WebhookEnvelope } from "./types.ts";

/**
 * LOCAL STAND-IN FOR VSMS.
 *
 * This is not a vsms simulator in any general sense, and must not be read
 * as one. Outbound webhook delivery is entirely unbuilt in vsms today
 * (`drain`/`hooks` worker roles are idle stubs; #38–#42 are open) — nothing
 * here has ever talked to a real vsms, and this emitter's only job is to
 * drive the three specific sequences #150 requires this example to
 * demonstrate against a real HTTP receiver:
 *
 *   1. the same event delivered twice (at-least-once retry)
 *   2. `message.delivered` arriving before `message.submitted`
 *   3. a signature that doesn't verify
 *
 * (plus a bonus rotation check: a signature made with the *previous*
 * secret, proving the receiver's overlap-window handling live rather than
 * just asserting it in prose.)
 *
 * The signing it performs uses the same `computeSignature` the receiver
 * verifies with (signature.ts) — this emitter is not a second, drifting
 * reimplementation of that provisional scheme.
 */

export interface EmitterOptions {
  baseUrl: string;
  secret: string;
  prevSecret: string;
}

interface PostOutcome {
  status: number;
  body: unknown;
}

function makeEnvelope(
  type: string,
  messageId: string,
  data: Record<string, unknown> = {},
): WebhookEnvelope {
  return {
    id: randomUUID(),
    type,
    occurredAt: new Date().toISOString(),
    data: { messageId, appId: "demo-app", clientRef: "demo-clientref", ...data },
  };
}

async function post(
  baseUrl: string,
  envelope: WebhookEnvelope,
  signing: { secret: string } | { forged: true },
): Promise<PostOutcome> {
  const rawBody = Buffer.from(JSON.stringify(envelope), "utf8");
  const timestamp = String(Math.floor(Date.now() / 1000));

  const signatureHeader =
    "forged" in signing
      ? // Syntactically valid (hex, right length) but not derived from any
        // secret this receiver holds — the case a genuinely tampered or
        // misconfigured sender produces.
        `v1=${"0".repeat(64)}`
      : `v1=${computeSignature(signing.secret, timestamp, envelope.id, rawBody)}`;

  const res = await fetch(new URL("/webhooks/vsms", baseUrl), {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-sms-event": envelope.type,
      "x-sms-event-id": envelope.id,
      "x-sms-timestamp": timestamp,
      "x-sms-signature": signatureHeader,
    },
    body: rawBody,
  });
  const body: unknown = await res.json().catch(() => undefined);
  return { status: res.status, body };
}

export async function runEmitterScenarios(options: EmitterOptions): Promise<void> {
  const { baseUrl, secret, prevSecret } = options;
  const settle = () => sleep(120); // let the receiver's async work queue drain before we log/inspect

  console.log("\n=== local emitter (stand-in for vsms — vsms cannot send webhooks yet) ===");

  console.log("\n-- case 1: duplicate delivery (at-least-once retry of the same event) --");
  const msgA = randomUUID();
  const delivered = makeEnvelope("message.delivered", msgA, { state: "delivered" });
  const first = await post(baseUrl, delivered, { secret });
  await settle();
  const retry = await post(baseUrl, delivered, { secret }); // byte-identical resend
  await settle();
  console.log(`   first delivery   -> HTTP ${first.status}`);
  console.log(`   duplicate resend -> HTTP ${retry.status} (same event id, same signature)`);

  console.log("\n-- case 2: out-of-order arrival (delivered before submitted) --");
  const msgB = randomUUID();
  const deliveredB = makeEnvelope("message.delivered", msgB, { state: "delivered" });
  const submittedB = makeEnvelope("message.submitted", msgB, { state: "submitted" });
  const rDelivered = await post(baseUrl, deliveredB, { secret });
  await settle();
  const rSubmitted = await post(baseUrl, submittedB, { secret });
  await settle();
  console.log(`   delivered arrives first -> HTTP ${rDelivered.status}`);
  console.log(`   submitted arrives late  -> HTTP ${rSubmitted.status}`);

  console.log("\n-- bonus: endpoint survives a secret rotation (prevSecret still accepted) --");
  const cancelledB = makeEnvelope("message.cancelled", msgB, { state: "cancelled" });
  const rPrev = await post(baseUrl, cancelledB, { secret: prevSecret });
  await settle();
  console.log(`   signed with prevSecret -> HTTP ${rPrev.status}`);

  console.log("\n-- case 3: bad signature --");
  const msgC = randomUUID();
  const forged = makeEnvelope("message.delivered", msgC, { state: "delivered" });
  const rBad = await post(baseUrl, forged, { forged: true });
  await settle();
  console.log(
    `   forged signature -> HTTP ${rBad.status} (expect 401 — rejected before any processing)`,
  );
}
