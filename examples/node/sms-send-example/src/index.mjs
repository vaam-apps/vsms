#!/usr/bin/env node
// vsms integration example (Node.js): send one message through vsms
// using the official @vaam-apps/vsms-node SDK.
//
// There is no token exchange. The SDK signs every request with your Ed25519
// key (COSE_Sign1, bound to the route, the audience and a fresh nonce), sends it
// to POST {base-url}/$procs/sendMessage with the Idempotency-Key it was given,
// and verifies the gateway's answer against the two public keys you pinned. An
// answer that is not sealed, or does not verify, is an error whatever its status.
//
// One-time setup, by an operator with access to the gateway:
//
//   # Your key. The private half stays with you; vsms never sees it.
//   openssl genpkey -algorithm ed25519 -out vsms-signing-key.pem
//   node src/index.mjs --print-public-key --signing-key-path vsms-signing-key.pem
//   # -> register that public key for your app (the operator runs this):
//   sms-gateway register-client --app <slug> --public-key <that value> --scope sms:send --scope sms:read
//   # The gateway's two public keys, to pin (also the operator's):
//   sms-gateway server-keys --envelope-signing-key-file ... --envelope-next-signing-key-file ... > server-keys.txt
//
// Then:
//
//   node src/index.mjs \
//     --base-url http://127.0.0.1:8080 \
//     --signing-key-path vsms-signing-key.pem \
//     --server-keys-path server-keys.txt \
//     --to +237677123456 \
//     --sender-id VYMALO \
//     --body "Hello from the vsms Node example"
//
// Every flag also reads from an env var (VSMS_BASE_URL, VSMS_SIGNING_KEY_PATH,
// VSMS_SERVER_KEYS_PATH) so a real integration never has to hardcode a
// credential path in argv.

import { createPrivateKey, createPublicKey } from "node:crypto";
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { parseServerKeysReport, SdkError, VsmsClient } from "@vaam-apps/vsms-node";

function parseCli() {
  const { values } = parseArgs({
    options: {
      "base-url": { type: "string", default: process.env.VSMS_BASE_URL ?? "http://127.0.0.1:8080" },
      "signing-key-path": { type: "string", default: process.env.VSMS_SIGNING_KEY_PATH },
      "server-keys-path": { type: "string", default: process.env.VSMS_SERVER_KEYS_PATH },
      "print-public-key": { type: "boolean", default: false },
      to: { type: "string" },
      "sender-id": { type: "string" },
      body: { type: "string", default: "Hello from the vsms Node.js integration example" },
      "client-ref": { type: "string" },
      "idempotency-key": { type: "string" },
    },
  });

  const required = values["print-public-key"]
    ? ["signing-key-path"]
    : ["signing-key-path", "server-keys-path", "to", "sender-id"];
  const missing = required.filter((key) => !values[key]);
  if (missing.length > 0) {
    console.error(`missing required flag(s): ${missing.map((k) => `--${k}`).join(", ")}`);
    console.error(
      "usage: node src/index.mjs --signing-key-path <pem> --server-keys-path <file> --to <e164> " +
        "--sender-id <senderId> [--body <text>] [--client-ref <key>] " +
        "[--idempotency-key <key>] [--base-url <url>]\n" +
        "       node src/index.mjs --print-public-key --signing-key-path <pem>",
    );
    process.exit(1);
  }

  return {
    baseUrl: values["base-url"].replace(/\/+$/, ""),
    signingKeyPath: values["signing-key-path"],
    serverKeysPath: values["server-keys-path"],
    printPublicKey: values["print-public-key"],
    to: values.to,
    senderId: values["sender-id"],
    body: values.body,
    clientRef: values["client-ref"],
    idempotencyKey: values["idempotency-key"],
  };
}

async function main() {
  const cli = parseCli();

  if (cli.printPublicKey) {
    // The public half is derived from the key alone; nothing is sent and no
    // server keys are needed. It is the raw 32-byte Ed25519 point, as the
    // unpadded base64url `register-client --public-key` takes.
    const key = createPrivateKey(readFileSync(cli.signingKeyPath, "utf8"));
    console.log(createPublicKey(key).export({ format: "jwk" }).x);
    return;
  }

  const client = new VsmsClient({
    baseUrl: cli.baseUrl,
    keyPath: cli.signingKeyPath,
    serverKeys: parseServerKeysReport(readFileSync(cli.serverKeysPath, "utf8")),
  });

  let outcome;
  try {
    outcome = await client.sendMessage(
      {
        to: cli.to,
        senderId: cli.senderId,
        body: cli.body,
        clientRef: cli.clientRef,
      },
      {
        idempotencyKey: cli.idempotencyKey,
      },
    );
  } catch (err) {
    if (err instanceof SdkError) {
      if (err.isUnauthorized()) {
        console.log(
          "the gateway refused the signature (an unsigned 401). Check that this key's public half " +
            "is registered (--print-public-key) and not revoked, and that this machine's clock is " +
            "within five minutes of the server's.",
        );
        return;
      }
      if (err.isIdempotencyInFlight()) {
        console.log(
          "sendMessage returned 409 Conflict — another request with this --idempotency-key is still in flight.",
        );
        return;
      }
      if (err.isIdempotencyKeyConflict()) {
        console.log(
          "sendMessage returned 422 — this --idempotency-key was already used with a *different* request body.",
        );
        return;
      }
      if (err.isConflict()) {
        console.log(
          "sendMessage returned 409 Conflict — if --client-ref was passed, that clientRef was " +
            "already used on a prior send. This is clientRef's database-level dedupe doing " +
            `its job: ${err.message}`,
        );
        return;
      }
    }
    throw err;
  }

  if (outcome.idempotencyReplayed) {
    console.log(
      "Idempotency-Replayed: true — this is the cached response from the first call under " +
        "this --idempotency-key, not a new send",
    );
  }

  const sent = outcome.result;
  console.log(
    `sent: messageId=${sent.messageId} state=${sent.state} encoding=${sent.encoding} ` +
      `segments=${sent.segments} operator=${sent.operator} estimatedCostXaf=${sent.estimatedCostXaf}`,
  );

  // Prove the write actually landed — read it back through the REST surface
  const message = await client.getMessage(sent.messageId);
  console.log(
    `read back: id=${message.id} state=${message.state} providerMessageRef=${message.providerMessageRef}`,
  );
}

main().catch((error) => {
  console.error(error.message ?? error);
  process.exitCode = 1;
});
