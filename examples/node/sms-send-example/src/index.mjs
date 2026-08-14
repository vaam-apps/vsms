#!/usr/bin/env node
// vsms integration example (Node.js): send one message through vsms
// using the official @vymalo/vsms-node SDK.
//
// The SDK handles:
// 1. Reading the private key PEM.
// 2. Signing the RFC 7523 private_key_jwt assertion.
// 3. Exchanging it at POST {issuer}/token and caching the access token.
// 4. Calling POST {issuer}/$procs/sendMessage with Bearer auth and Idempotency-Key.
// 5. Bounded 401 retry on mid-flight key/token rotation.
//
// Usage:
//   node src/index.mjs \
//     --issuer http://127.0.0.1:8080 \
//     --client-id <clientId that provision-client printed> \
//     --private-key-path /path/to/console-client-key.pem \
//     --to +237677123456 \
//     --sender-id VYMALO \
//     --body "Hello from the vsms Node example"
//
// Every flag also reads from an env var (VSMS_ISSUER, VSMS_CLIENT_ID,
// VSMS_PRIVATE_KEY_PATH, VSMS_SCOPE).

import { parseArgs } from "node:util";
import { SdkError, VsmsClient } from "@vymalo/vsms-node";

function parseCli() {
  const { values } = parseArgs({
    options: {
      issuer: { type: "string", default: process.env.VSMS_ISSUER ?? "http://127.0.0.1:8080" },
      "client-id": { type: "string", default: process.env.VSMS_CLIENT_ID },
      "private-key-path": { type: "string", default: process.env.VSMS_PRIVATE_KEY_PATH },
      scope: { type: "string", default: process.env.VSMS_SCOPE ?? "sms:send sms:read" },
      to: { type: "string" },
      "sender-id": { type: "string" },
      body: { type: "string", default: "Hello from the vsms Node.js integration example" },
      "client-ref": { type: "string" },
      "idempotency-key": { type: "string" },
    },
  });

  const missing = ["client-id", "private-key-path", "to", "sender-id"].filter(
    (key) => !values[key],
  );
  if (missing.length > 0) {
    console.error(`missing required flag(s): ${missing.map((k) => `--${k}`).join(", ")}`);
    console.error(
      "usage: node src/index.mjs --client-id <id> --private-key-path <pem> --to <e164> " +
        "--sender-id <senderId> [--body <text>] [--client-ref <key>] " +
        "[--idempotency-key <key>] [--issuer <url>] [--scope <scopes>]",
    );
    process.exit(1);
  }

  return {
    issuer: values.issuer.replace(/\/+$/, ""),
    clientId: values["client-id"],
    privateKeyPath: values["private-key-path"],
    scope: values.scope,
    to: values.to,
    senderId: values["sender-id"],
    body: values.body,
    clientRef: values["client-ref"],
    idempotencyKey: values["idempotency-key"],
  };
}

async function main() {
  const cli = parseCli();

  const client = VsmsClient.privateKeyJwt({
    issuer: cli.issuer,
    clientId: cli.clientId,
    keyPath: cli.privateKeyPath,
    scope: cli.scope,
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
