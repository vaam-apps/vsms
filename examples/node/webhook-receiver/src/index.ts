import { readFileSync } from "node:fs";
import type { Server } from "node:http";
import { fileURLToPath } from "node:url";
import { WebhookOpener } from "@vaam-apps/vsms-node";
import { ROUTE, sealedOpener } from "./config.ts";
import { HmacOpener } from "./hmac-opener.ts";
import { createReceiver } from "./server.ts";
import { computeSignature } from "./signature.ts";
import type { ProcessedResult } from "./types.ts";

const PORT = Number(process.env.WEBHOOK_RECEIVER_PORT ?? 4790);

// The one genuine delivery this demo can show without a stack: bytes the
// gateway's own `sms-webhook` crate sealed (`tests/sealed_vector.rs` writes
// it), signed by a throwaway test key. The demo pins that key's public half
// and fixes its clock at the delivery's own timestamp so the recording is
// still "fresh". A real deployment pins `sms-gateway server-keys`' output
// (see `config.ts`) and reads the real clock. A recording cannot show a replay
// being refused: the nonce memory expires an entry against the real clock, and
// the pinned one is far behind it. The SDKs' own tests seal fresh deliveries
// and prove that.
const VECTOR = JSON.parse(
  readFileSync(
    fileURLToPath(
      new URL(
        "../../../../sdks/rust/vsms-sdk-rust/tests/fixtures/sealed_delivery_vector.json",
        import.meta.url,
      ),
    ),
    "utf8",
  ),
) as {
  publicKeyHex: string;
  audience: string;
  route: string;
  appId: string;
  endpointId: string;
  iat: number;
  sealedHex: string;
};

console.log("============================================================");
console.log(" vsms reference webhook receiver (examples/node/webhook-receiver)");
console.log("============================================================");
console.log(" An endpoint is delivered to in one of two modes (deliveryMode):");
console.log("   sealed - a request signed with the gateway's own key, opened with");
console.log("            @vaam-apps/vsms-node's WebhookOpener. No shared secret.");
console.log("   hmac   - X-Sms-Signature under the endpoint's own secret, the");
console.log("            original scheme, for receivers that have not moved yet.");
console.log(" This demo runs one receiver of each. See README.md.");
console.log("------------------------------------------------------------");
console.log(` listening on   : http://localhost:${PORT}`);
console.log(` webhook route  : POST ${ROUTE} (sealed on ${PORT}, hmac on ${PORT + 1})`);
console.log(" health route   : GET  /healthz");
console.log("============================================================\n");

const results: ProcessedResult[] = [];

// The vector's route is the example's own, so the demo's receiver serves it.
if (VECTOR.route !== ROUTE) {
  throw new Error(`the vector is sealed for ${VECTOR.route}, this receiver serves ${ROUTE}`);
}

const opener = new WebhookOpener({
  serverKeys: {
    current: Buffer.from(VECTOR.publicKeyHex, "hex").toString("base64url"),
    // A second key, as a real deployment pins (`next`): RFC 8032's test vector 2,
    // never used to sign here.
    next: "PUAXw-hDiVqStwqnTRt-vJyYLM8uxJaMwM1V8Sr0Zgw",
  },
  audience: VECTOR.audience,
  route: ROUTE,
  // The app the recorded endpoint belongs to; a real receiver reads its own
  // from configuration (`config.ts`).
  appId: VECTOR.appId,
  endpointId: VECTOR.endpointId,
  nowSeconds: VECTOR.iat,
});

const receiver = createReceiver({
  port: PORT,
  opener: sealedOpener(opener),
  onProcessed: (result) => {
    results.push(result);
    console.log(`[receiver] ${result.status.padEnd(28)} ${result.detail}`);
  },
});

async function post(body: Uint8Array): Promise<number> {
  const response = await fetch(`http://localhost:${PORT}${ROUTE}`, {
    method: "POST",
    headers: { "content-type": 'application/cose; cose-type="cose-sign1"' },
    body: new Uint8Array(body),
  });
  return response.status;
}

// The same receiver logic behind the other mode's opener, on the next port. Its
// secrets are demo values: a real `hmac` receiver reads `WebhookEndpoint.secret`
// (and `prevSecret` while a rotation is open) from wherever it keeps secrets.
const HMAC_PORT = PORT + 1;
const HMAC_SECRET = "demo-current-secret-do-not-use-in-prod";
const HMAC_PREV_SECRET = "demo-previous-secret-do-not-use-in-prod";
const hmacResults: ProcessedResult[] = [];
const hmacReceiver = createReceiver({
  port: HMAC_PORT,
  opener: new HmacOpener(HMAC_SECRET, HMAC_PREV_SECRET),
  onProcessed: (result) => {
    hmacResults.push(result);
    console.log(`[receiver:hmac] ${result.status.padEnd(28)} ${result.detail}`);
  },
});

/** An `hmac` delivery as `sms-worker` sends one: the four-field envelope and the four `X-Sms-*` headers. */
async function postHmac(
  eventId: string,
  type: string,
  messageId: string,
  secret: string | "forged",
): Promise<number> {
  const body = Buffer.from(
    JSON.stringify({
      id: `attempt-${eventId}`,
      type,
      occurredAt: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
      data: { messageId },
    }),
    "utf8",
  );
  const timestamp = String(Math.floor(Date.now() / 1000));
  const signature =
    secret === "forged"
      ? `v1=${"0".repeat(64)}`
      : `v1=${computeSignature(secret, timestamp, eventId, body)}`;
  const response = await fetch(`http://localhost:${HMAC_PORT}${ROUTE}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-sms-event": type,
      "x-sms-event-id": eventId,
      "x-sms-timestamp": timestamp,
      "x-sms-signature": signature,
    },
    body,
  });
  return response.status;
}

let server: Server | undefined;
let hmacServer: Server | undefined;

try {
  server = await receiver.listen();
  const genuine = Buffer.from(VECTOR.sealedHex, "hex");

  console.log("-- a genuine delivery (bytes the gateway's own sealer produced) --");
  console.log(
    `   HTTP ${await post(genuine)}  (expect 202: opened, acked, processed off the request path)`,
  );
  await new Promise((resolve) => setTimeout(resolve, 200));

  console.log("-- one flipped bit --");
  const tampered = Buffer.from(genuine);
  tampered[tampered.length - 1] = (tampered[tampered.length - 1] ?? 0) ^ 1;
  console.log(`   HTTP ${await post(tampered)}  (expect 401: the signature no longer verifies)`);

  await new Promise((resolve) => setTimeout(resolve, 200));

  hmacServer = await hmacReceiver.listen();
  console.log("\n-- an hmac endpoint (the original scheme, signed with the endpoint's secret) --");
  const hmacEvent = "11111111-1111-4111-8111-111111111111";
  console.log(
    `   HTTP ${await postHmac(hmacEvent, "message.delivered", "msg-hmac-1", HMAC_SECRET)}  (expect 202)`,
  );
  await new Promise((resolve) => setTimeout(resolve, 200));
  console.log(
    `   HTTP ${await postHmac(hmacEvent, "message.delivered", "msg-hmac-1", HMAC_SECRET)}  (expect 202: a retry, acked and deduped on X-Sms-Event-Id)`,
  );
  await new Promise((resolve) => setTimeout(resolve, 200));
  console.log(
    `   HTTP ${await postHmac("22222222-2222-4222-8222-222222222222", "message.submitted", "msg-hmac-2", HMAC_PREV_SECRET)}  (expect 202: signed with the previous secret, inside a rotation's overlap window)`,
  );
  await new Promise((resolve) => setTimeout(resolve, 200));
  console.log(
    `   HTTP ${await postHmac("33333333-3333-4333-8333-333333333333", "message.delivered", "msg-hmac-3", "forged")}  (expect 401: a signature no held secret produces)`,
  );
  await new Promise((resolve) => setTimeout(resolve, 200));

  console.log("\n=== summary ===");
  const counts = results.reduce<Record<string, number>>((acc, r) => {
    acc[r.status] = (acc[r.status] ?? 0) + 1;
    return acc;
  }, {});
  for (const [status, count] of Object.entries(counts)) {
    console.log(`  sealed ${status.padEnd(28)} x${count}`);
  }
  const hmacCounts = hmacResults.reduce<Record<string, number>>((acc, r) => {
    acc[r.status] = (acc[r.status] ?? 0) + 1;
    return acc;
  }, {});
  for (const [status, count] of Object.entries(hmacCounts)) {
    console.log(`  hmac   ${status.padEnd(28)} x${count}`);
  }
  console.log(
    "\nDemo complete. A retry of an event arrives as a NEW sealed (or freshly signed) request " +
      "carrying the same id: that is what the receiver dedupes on (see server.ts and store.ts).",
  );
} finally {
  server?.close();
  hmacServer?.close();
}
