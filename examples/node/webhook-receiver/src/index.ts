import { readFileSync } from "node:fs";
import type { Server } from "node:http";
import { fileURLToPath } from "node:url";
import { WebhookOpener } from "@vaam-apps/vsms-node";
import { ROUTE } from "./config.ts";
import { createReceiver } from "./server.ts";
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
console.log(" A delivery is a sealed request signed with the gateway's own key.");
console.log(" This receiver opens it with @vaam-apps/vsms-node's WebhookOpener:");
console.log(" no shared secret, no signature header. See README.md.");
console.log("------------------------------------------------------------");
console.log(` listening on   : http://localhost:${PORT}`);
console.log(` webhook route  : POST ${ROUTE}`);
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
  opener,
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

let server: Server | undefined;

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
  console.log("\n=== summary ===");
  const counts = results.reduce<Record<string, number>>((acc, r) => {
    acc[r.status] = (acc[r.status] ?? 0) + 1;
    return acc;
  }, {});
  for (const [status, count] of Object.entries(counts)) {
    console.log(`  ${status.padEnd(28)} x${count}`);
  }
  console.log(
    "\nDemo complete. A retry of this event would arrive as a NEW sealed request carrying the " +
      "same id: that is what the receiver dedupes on (see server.ts and store.ts).",
  );
} finally {
  server?.close();
}
