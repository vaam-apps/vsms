/**
 * The demo showcase's own evaluator, not just another integration example.
 *
 * Every other `examples/node/*` package proves ONE half of a vsms
 * integration in isolation — `sms-send-example` sends, `webhook-receiver`
 * receives (against a local, fabricated emitter). This package is what
 * `compose.dev.yaml`/`compose.demo.yaml`'s `demo-app` service runs: it does
 * both, against the *real* stack those compose files bring up, and reports
 * pass/fail on the one property an evaluator actually cares about —
 * "did a message I sent really get delivered, and did I really get told
 * about it, with a signature that really verifies?"
 *
 * What this proves, end to end, over real HTTP, against real containers:
 *
 *   1. `private_key_jwt` token acquisition works (`@vymalo/vsms-node`,
 *      the same SDK an external integrator would use — not a shortcut).
 *   2. `sendMessage` accepts a real OTP send.
 *   3. The message is actually routed, submitted (to `sms-fake-orange`,
 *      never a real carrier — see that binary's own module doc), and
 *      reaches a real terminal state via a real DLR round trip.
 *   4. vsms's `hooks` worker role really signs and POSTs a webhook for
 *      that message back to *this* process's own `POST /webhooks` route,
 *      and the signature genuinely verifies — `signature.ts` (copied
 *      verbatim from `examples/node/webhook-receiver`, see
 *      `verbatim-copy.test.ts`) is exactly the code a real integrator's
 *      receiver would run.
 *
 * Exit code is the whole point: `0` only if the message reached
 * `delivered` AND at least one webhook for it verified. Anything else is
 * a loud, non-zero failure naming exactly what didn't happen — this is
 * meant to be watched in `docker compose logs demo-app` (or `just
 * demo-app`), not just left to run.
 *
 * # Credential reuse, deliberately not a second provisioned client
 *
 * This process authenticates as the *same* machine credential
 * `compose.dev.yaml`/`compose.demo.yaml`'s own `provision-client` step
 * already provisions for the admin console (`vsms_*_secrets` volume,
 * `/secrets/console-client-key.pem` + `/secrets/console-client-id`) — not
 * a second, separately-provisioned `AppClient`. That credential already
 * carries `sms:send`/`sms:read` (among other scopes `admin` needs), so a
 * second client would duplicate provisioning for no capability this one
 * lacks. Reusing it also means a real evaluator watching `docker compose
 * logs -f` sees the *same* client id show up in the admin console's own
 * Messages screen, not an unexplained second identity.
 */

import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { setTimeout as sleep } from "node:timers/promises";
import type { Message, MessageState } from "@vymalo/vsms-node";
import { SdkError, VsmsClient } from "@vymalo/vsms-node";
import type { Request, Response } from "express";
import express from "express";
import { verifySignature } from "./signature.ts";

// ---------------------------------------------------------------------------
// Configuration — every value has a default matching what
// compose.dev.yaml/compose.demo.yaml's own `demo-app` service passes, so a
// human can also just `cd examples/node/demo-app && pnpm install && pnpm
// start` against a `just demo` stack with nothing set beyond VSMS_ISSUER.
// ---------------------------------------------------------------------------

function envInt(name: string, fallback: number): number {
  const raw = process.env[name];
  if (!raw) return fallback;
  const parsed = Number.parseInt(raw, 10);
  if (Number.isNaN(parsed)) {
    throw new Error(`${name}=${raw} is not a valid integer`);
  }
  return parsed;
}

function readFileTrimmed(path: string): string {
  return readFileSync(path, "utf8").trim();
}

const ISSUER = (process.env.VSMS_ISSUER ?? "http://sms-gateway:8080").replace(/\/+$/, "");
const SCOPE = process.env.VSMS_SCOPE ?? "sms:send sms:read";
const CLIENT_ID_PATH = process.env.VSMS_CLIENT_ID_PATH ?? "/secrets/console-client-id";
const PRIVATE_KEY_PATH = process.env.VSMS_PRIVATE_KEY_PATH ?? "/secrets/console-client-key.pem";

const WEBHOOK_PORT = envInt("DEMO_WEBHOOK_PORT", 9000);
const WEBHOOK_SECRET_PATH = process.env.DEMO_WEBHOOK_SECRET_PATH ?? "/secrets/webhook-secret";

const DEMO_TO = process.env.DEMO_TO ?? "+237677123456";
const DEMO_SENDER_ID = process.env.DEMO_SENDER_ID ?? "VSMS";
const DEMO_BODY = process.env.DEMO_BODY ?? "Your vsms demo OTP is 482913. It expires in 5 minutes.";

const OVERALL_TIMEOUT_MS = envInt("DEMO_TIMEOUT_MS", 90_000);
const SEND_RETRY_INTERVAL_MS = 2_000;
const POLL_INTERVAL_MS = 1_000;
// hooks.rs polls its own claim loop every 1s (POLL_INTERVAL, see that
// file's own const) — this is slack on top of a message reaching a
// terminal state for the trailing webhook(s) still in flight to arrive,
// not a guess at an unknown cadence.
const WEBHOOK_SETTLE_MS = 8_000;

/**
 * A message is done moving, for this demo's purposes, the moment it
 * reaches any state §7.4 calls terminal (`purge_retention`'s own
 * candidate set: delivered/failed/expired/rejected/cancelled) or
 * `undelivered` — AGENTS.md's own #122 entry documents that nothing
 * currently drives `undelivered` any further within this deployment's
 * scope, so waiting past it here would just burn the whole timeout.
 * `uncertain` is deliberately NOT included — #119's own design lets a
 * message leave `uncertain` later (via a DLR or `expire_stale`'s 6h
 * grace), so stopping there would report a false negative for a message
 * that is, correctly, still being given a chance to resolve. In practice
 * `sms-fake-orange`'s default fault policy essentially never produces
 * `uncertain` for a fresh demo send.
 */
const TERMINAL_STATES: ReadonlySet<MessageState> = new Set([
  "delivered",
  "failed",
  "expired",
  "rejected",
  "cancelled",
  "undelivered",
]);

// ---------------------------------------------------------------------------
// Inbound half: a minimal receiver, `signature.ts`'s verifySignature is the
// entire security-relevant logic — same function a real integrator's own
// receiver would call.
// ---------------------------------------------------------------------------

interface ReceivedWebhook {
  type: string;
  eventId: string;
  verified: boolean;
  reason: string | undefined;
  receivedAtMs: number;
  messageId: string | undefined;
  dataState: string | undefined;
}

function startWebhookServer(port: number, secret: string, events: ReceivedWebhook[]) {
  const app = express();

  // Raw bytes, not `express.json()` — verification is over the exact
  // bytes vsms signed, and a JSON-parse-then-reserialize round trip is
  // not guaranteed to reproduce them byte-for-byte (`hooks.rs`'s own
  // module doc makes the identical point about signing vs. sending).
  app.post(
    "/webhooks",
    express.raw({ type: "*/*", limit: "1mb" }),
    (req: Request, res: Response) => {
      const rawBody = req.body instanceof Buffer ? req.body : Buffer.alloc(0);
      const result = verifySignature({
        rawBody,
        timestamp: req.header("x-sms-timestamp"),
        eventId: req.header("x-sms-event-id"),
        signatureHeader: req.header("x-sms-signature"),
        secrets: [secret],
      });

      let messageId: string | undefined;
      let dataState: string | undefined;
      try {
        const parsed = JSON.parse(rawBody.toString("utf8")) as {
          data?: { messageId?: unknown; state?: unknown };
        };
        messageId = typeof parsed.data?.messageId === "string" ? parsed.data.messageId : undefined;
        dataState = typeof parsed.data?.state === "string" ? parsed.data.state : undefined;
      } catch {
        // A body that doesn't even parse as JSON still gets recorded below,
        // as an unverified/unattributable event — signature.ts's own
        // verification already ran against the raw bytes regardless.
      }

      const event: ReceivedWebhook = {
        type: req.header("x-sms-event") ?? "unknown",
        eventId: req.header("x-sms-event-id") ?? "unknown",
        verified: result.ok,
        reason: result.ok ? undefined : result.reason,
        receivedAtMs: Date.now(),
        messageId,
        dataState,
      };
      events.push(event);

      if (result.ok) {
        console.log(
          `[webhook] VERIFIED  type=${event.type} messageId=${event.messageId ?? "?"} state=${event.dataState ?? "?"}`,
        );
        res.status(200).json({ ok: true });
      } else {
        console.error(
          `[webhook] SIGNATURE VERIFICATION FAILED  type=${event.type} messageId=${event.messageId ?? "?"} reason=${event.reason}`,
        );
        res
          .status(401)
          .json({ ok: false, error: "signature verification failed", reason: event.reason });
      }
    },
  );

  app.get("/healthz", (_req: Request, res: Response) => {
    res.status(200).json({ ok: true });
  });

  const server = createServer(app);
  return new Promise<typeof server>((resolve) => {
    server.listen(port, "0.0.0.0", () => {
      console.log(`[demo-app] webhook receiver listening on :${port} (POST /webhooks)`);
      resolve(server);
    });
  });
}

// ---------------------------------------------------------------------------
// Outbound half: send, then poll until terminal.
// ---------------------------------------------------------------------------

async function sendWithRetry(client: VsmsClient, deadlineMs: number) {
  let lastError: unknown;
  while (Date.now() < deadlineMs) {
    try {
      return await client.sendMessage(
        { to: DEMO_TO, senderId: DEMO_SENDER_ID, body: DEMO_BODY, class: "otp" },
        {},
      );
    } catch (err) {
      lastError = err;
      const message = err instanceof Error ? err.message : String(err);
      console.log(`[demo-app] sendMessage not ready yet (${message}) — retrying...`);
      await sleep(SEND_RETRY_INTERVAL_MS);
    }
  }
  throw lastError instanceof Error
    ? lastError
    : new Error(`sendMessage never succeeded before the ${OVERALL_TIMEOUT_MS}ms deadline`);
}

async function pollUntilTerminal(
  client: VsmsClient,
  messageId: string,
  startedAtMs: number,
  deadlineMs: number,
): Promise<{ timeline: Array<{ state: MessageState; atMs: number }>; final: Message | undefined }> {
  const timeline: Array<{ state: MessageState; atMs: number }> = [];
  let lastState: MessageState | undefined;

  while (Date.now() < deadlineMs) {
    let message: Message;
    try {
      message = await client.getMessage(messageId);
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      console.log(`[demo-app] getMessage failed transiently (${detail}) — retrying...`);
      await sleep(POLL_INTERVAL_MS);
      continue;
    }

    if (message.state !== lastState) {
      const atMs = Date.now();
      timeline.push({ state: message.state, atMs });
      console.log(`[demo-app] state=${message.state}  (+${atMs - startedAtMs}ms)`);
      lastState = message.state;
    }

    if (TERMINAL_STATES.has(message.state)) {
      return { timeline, final: message };
    }

    await sleep(POLL_INTERVAL_MS);
  }

  return { timeline, final: undefined };
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

async function main(): Promise<number> {
  const secret = process.env.DEMO_WEBHOOK_SECRET ?? readFileTrimmed(WEBHOOK_SECRET_PATH);
  const clientId = process.env.VSMS_CLIENT_ID ?? readFileTrimmed(CLIENT_ID_PATH);

  const events: ReceivedWebhook[] = [];
  const server = await startWebhookServer(WEBHOOK_PORT, secret, events);

  const deadlineMs = Date.now() + OVERALL_TIMEOUT_MS;

  console.log(
    `[demo-app] issuer=${ISSUER} clientId=${clientId} to=${DEMO_TO} senderId=${DEMO_SENDER_ID}`,
  );

  const client = VsmsClient.privateKeyJwt({
    issuer: ISSUER,
    clientId,
    keyPath: PRIVATE_KEY_PATH,
    scope: SCOPE,
  });

  let exitCode = 1;
  try {
    const outcome = await sendWithRetry(client, deadlineMs);
    const sent = outcome.result;
    const startedAtMs = Date.now();
    console.log(
      `[demo-app] accepted messageId=${sent.messageId} state=${sent.state} encoding=${sent.encoding} ` +
        `segments=${sent.segments} operator=${sent.operator} estimatedCostXaf=${sent.estimatedCostXaf}`,
    );

    const { timeline, final } = await pollUntilTerminal(
      client,
      sent.messageId,
      startedAtMs,
      deadlineMs,
    );

    // Give the last webhook(s) — most importantly the one for the
    // terminal state we just observed — a chance to actually arrive
    // before judging the run. `hooks` polls every 1s; this is generous
    // slack on top of that, bounded by the overall deadline regardless.
    const settleDeadline = Math.min(deadlineMs, Date.now() + WEBHOOK_SETTLE_MS);
    while (Date.now() < settleDeadline && !events.some((e) => e.verified)) {
      await sleep(500);
    }
    // A short, fixed grace period even after a verified event shows up,
    // so a *second* in-flight event (e.g. message.submitted arriving
    // just after message.delivered did) isn't cut off mid-flight.
    await sleep(1_000);

    const verifiedCount = events.filter((e) => e.verified).length;
    const delivered = final?.state === "delivered";

    console.log("");
    console.log("=== timeline ===");
    for (const step of timeline) {
      console.log(`  +${String(step.atMs - startedAtMs).padStart(6)}ms  ${step.state}`);
    }
    console.log("");
    console.log("=== webhook events received ===");
    if (events.length === 0) {
      console.log("  (none)");
    }
    for (const event of events) {
      const status = event.verified ? "verified" : `UNVERIFIED (${event.reason})`;
      console.log(
        `  +${String(event.receivedAtMs - startedAtMs).padStart(6)}ms  ${event.type} messageId=${event.messageId ?? "?"} — ${status}`,
      );
    }
    console.log("");

    if (delivered && verifiedCount >= 1) {
      console.log(
        `[demo-app] SUCCESS: messageId=${sent.messageId} reached delivered with ${verifiedCount} verified webhook(s) of ${events.length} received`,
      );
      exitCode = 0;
    } else {
      const reasons: string[] = [];
      if (!delivered) {
        reasons.push(
          `message never reached delivered (final observed state: ${final?.state ?? "timed out waiting"})`,
        );
      }
      if (verifiedCount === 0) {
        reasons.push(
          events.length === 0
            ? "no webhook was received at all"
            : `${events.length} webhook(s) were received but NONE verified their signature — check that WEBHOOK secret matches the seeded WebhookEndpoint`,
        );
      }
      console.error(`[demo-app] FAILURE: messageId=${sent.messageId} — ${reasons.join("; ")}`);
      exitCode = 1;
    }
  } catch (err) {
    if (err instanceof SdkError) {
      console.error(
        `[demo-app] FAILURE: SdkError: ${err.message} (httpStatus=${err.httpStatus ?? "n/a"})`,
      );
    } else {
      console.error(`[demo-app] FAILURE: ${err instanceof Error ? err.stack : err}`);
    }
    exitCode = 1;
  }

  await new Promise<void>((resolve) => server.close(() => resolve()));
  return exitCode;
}

main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((err) => {
    console.error("[demo-app] FATAL, uncaught:", err);
    process.exitCode = 1;
  });
