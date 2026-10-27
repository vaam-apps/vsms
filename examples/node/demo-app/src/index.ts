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
 * about it, with a delivery that really opens under the gateway's key?"
 *
 * What this proves, end to end, over real HTTP, against real containers:
 *
 *   1. A signed request is accepted (`@vaam-apps/vsms-node`, the same SDK
 *      an external integrator would use — not a shortcut): every request is
 *      a COSE_Sign1 envelope under this app's own Ed25519 key, and every
 *      answer is verified against the gateway's two pinned keys.
 *   2. `sendMessage` accepts a real OTP send.
 *   3. The message is actually routed, submitted (to `sms-fake-orange`,
 *      never a real carrier — see that binary's own module doc), and
 *      reaches a real terminal state via a real DLR round trip.
 *   4. vsms's `hooks` worker role really seals and POSTs a webhook for
 *      that message back to *this* process's own `POST /webhooks` route,
 *      and the delivery genuinely opens — `WebhookOpener` from the same SDK
 *      (the gateway's own envelope signing key, pinned from
 *      `dev-keys/server-keys.txt`, for the audience this app's endpoint was
 *      created with) is exactly the code a real integrator's receiver
 *      would run.
 *
 * Exit code is the whole point: `0` only if the message reached
 * `delivered` AND at least one webhook for it verified. Anything else is
 * a loud, non-zero failure naming exactly what didn't happen — this is
 * meant to be watched in `docker compose logs demo-app` (or `just
 * demo-app`), not just left to run. That decision itself lives in
 * `./decision.ts`, deliberately not inline here — see that module's own
 * doc comment for why (a sabotaged inline predicate passed every test
 * this package had before the extraction).
 *
 * # Its own key
 *
 * This process signs as its own `AppClient`, registered by the compose
 * stacks' `register-demo-app` step from the public half of
 * `dev-keys/demo-app.pem` (`just dev-keys` mints it), with the two scopes it
 * uses, `sms:send` and `sms:read`. The gateway's two public sealing keys it
 * pins come from `dev-keys/server-keys.txt`, the same two values
 * `sms-gateway server-keys` prints. Nothing is acquired or refreshed: there is
 * no token.
 */

import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { setTimeout as sleep } from "node:timers/promises";
import type { Message, MessageState } from "@vaam-apps/vsms-node";
import {
  parseServerKeysReport,
  SdkError,
  VsmsClient,
  type WebhookEvent,
  WebhookOpener,
} from "@vaam-apps/vsms-node";
import type { Request, Response } from "express";
import express from "express";
import { decide } from "./decision.ts";

// ---------------------------------------------------------------------------
// Configuration — every value has a default matching what
// compose.dev.yaml/compose.demo.yaml's own `demo-app` service passes, so a
// human can also just `cd examples/node/demo-app && pnpm install && pnpm
// start` against a `just demo` stack with nothing set beyond VSMS_BASE_URL
// (the container-network address is the default; a host process points it at
// the published port) and the two key paths.
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

const BASE_URL = (process.env.VSMS_BASE_URL ?? "http://sms-gateway:8080").replace(/\/+$/, "");
const SIGNING_KEY_PATH = process.env.VSMS_SIGNING_KEY_PATH ?? "/run/demo/demo-app.pem";
const SERVER_KEYS_PATH = process.env.VSMS_SERVER_KEYS_PATH ?? "/run/demo/server-keys.txt";

const WEBHOOK_PORT = envInt("DEMO_WEBHOOK_PORT", 9000);
// The audience the demo's `WebhookEndpoint` was created with
// (`vsms-demo-seed --webhook-audience`; the compose stacks pass `demo-app`).
// The delivery is addressed to it, and the opener refuses one that is not.
const WEBHOOK_AUDIENCE = process.env.DEMO_WEBHOOK_AUDIENCE ?? "demo-app";
// The id of the app the endpoint belongs to. Unset, the demo learns it from the
// verified answer to its own `getMessage` (see the inbound half below); set, it
// is used as given, which is how a real receiver is configured.
const WEBHOOK_APP_ID = process.env.DEMO_WEBHOOK_APP_ID || undefined;
// The path of `WebhookEndpoint.url` (`http://demo-app:9000/webhooks`).
const WEBHOOK_ROUTE = "/webhooks";

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
// Inbound half: a minimal receiver. `WebhookOpener.open` is the entire
// security-relevant logic: the same call a real integrator's own receiver
// makes. It needs the id of the app its endpoint belongs to, because the
// gateway signs every app's deliveries with one key; a real receiver reads that
// from its configuration. This demo learns it from the one place it can trust
// without being told: the verified answer to its own `getMessage`, which names
// the app the message (and so the endpoint) belongs to.
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

function startWebhookServer(
  port: number,
  opener: Promise<WebhookOpener>,
  events: ReceivedWebhook[],
) {
  const app = express();

  // Raw bytes, not `express.json()`: the body is a COSE_Sign1 message, and
  // the opener verifies exactly the bytes vsms sent.
  app.post(
    WEBHOOK_ROUTE,
    express.raw({ type: () => true, limit: "1mb" }),
    async (req: Request, res: Response) => {
      const rawBody = req.body instanceof Buffer ? req.body : Buffer.alloc(0);

      let opened: WebhookEvent | undefined;
      let failure: string | undefined;
      try {
        opened = await (await opener).open(rawBody);
      } catch (err) {
        failure = err instanceof Error ? err.message : String(err);
      }

      const data = opened?.data as { messageId?: unknown; state?: unknown } | undefined;
      const event: ReceivedWebhook = {
        type: opened?.type ?? "unknown",
        eventId: opened?.id ?? "unknown",
        verified: opened !== undefined,
        reason: failure,
        receivedAtMs: Date.now(),
        messageId: typeof data?.messageId === "string" ? data.messageId : undefined,
        dataState: typeof data?.state === "string" ? data.state : undefined,
      };
      events.push(event);

      if (opened !== undefined) {
        console.log(
          `[webhook] VERIFIED  type=${event.type} messageId=${event.messageId ?? "?"} state=${event.dataState ?? "?"}`,
        );
        res.status(200).json({ ok: true });
      } else {
        console.error(`[webhook] DELIVERY NOT OPENED  reason=${event.reason}`);
        res.status(401).json({ ok: false, error: "delivery not verified" });
      }
    },
  );

  app.get("/healthz", (_req: Request, res: Response) => {
    res.status(200).json({ ok: true });
  });

  const server = createServer(app);
  return new Promise<typeof server>((resolve) => {
    server.listen(port, "0.0.0.0", () => {
      console.log(`[demo-app] webhook receiver listening on :${port} (POST ${WEBHOOK_ROUTE})`);
      resolve(server);
    });
  });
}

// ---------------------------------------------------------------------------
// Outbound half: send, then poll until terminal.
// ---------------------------------------------------------------------------

/**
 * The SDK bounds each call with its own `timeoutMs` (30s by default) but knows
 * nothing of this process's overall `DEMO_TIMEOUT_MS` budget, and a sealed call
 * is never retried by the SDK. `Promise.race`d against the remaining deadline,
 * a call started late in the budget cannot outlive it: a client-side bound, not
 * a cancellation of the request.
 */
function withDeadline<T>(promise: Promise<T>, deadlineMs: number, label: string): Promise<T> {
  const remainingMs = Math.max(deadlineMs - Date.now(), 0);
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(
        new Error(
          `${label} exceeded the remaining ${remainingMs}ms budget (deadline reached) — ` +
            "this is a client-side bound on the overall budget, not a cancellation of the request",
        ),
      );
    }, remainingMs);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (err: unknown) => {
        clearTimeout(timer);
        reject(err instanceof Error ? err : new Error(String(err)));
      },
    );
  });
}

async function sendWithRetry(client: VsmsClient, deadlineMs: number) {
  let lastError: unknown;
  while (Date.now() < deadlineMs) {
    try {
      return await withDeadline(
        client.sendMessage(
          { to: DEMO_TO, senderId: DEMO_SENDER_ID, body: DEMO_BODY, class: "otp" },
          {},
        ),
        deadlineMs,
        "sendMessage",
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
      message = await withDeadline(client.getMessage(messageId), deadlineMs, "getMessage");
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
  // The gateway's two public keys: the ones it seals its answers with, and
  // the very ones that sign a webhook delivery. One pin for both.
  const serverKeys = parseServerKeysReport(readFileTrimmed(SERVER_KEYS_PATH));
  let learnedAppId!: (appId: string) => void;
  const opener = new Promise<WebhookOpener>((resolve) => {
    learnedAppId = (appId) =>
      resolve(
        new WebhookOpener({
          serverKeys,
          audience: WEBHOOK_AUDIENCE,
          route: WEBHOOK_ROUTE,
          appId,
        }),
      );
  });
  if (WEBHOOK_APP_ID !== undefined) learnedAppId(WEBHOOK_APP_ID);

  const events: ReceivedWebhook[] = [];
  const server = await startWebhookServer(WEBHOOK_PORT, opener, events);

  const deadlineMs = Date.now() + OVERALL_TIMEOUT_MS;

  console.log(`[demo-app] baseUrl=${BASE_URL} to=${DEMO_TO} senderId=${DEMO_SENDER_ID}`);

  const client = new VsmsClient({
    baseUrl: BASE_URL,
    keyPath: SIGNING_KEY_PATH,
    serverKeys,
  });
  console.log(`[demo-app] signing as the registered key ${client.publicKey}`);

  let exitCode = 1;
  try {
    const outcome = await sendWithRetry(client, deadlineMs);
    const sent = outcome.result;
    const startedAtMs = Date.now();
    console.log(
      `[demo-app] accepted messageId=${sent.messageId} state=${sent.state} encoding=${sent.encoding} ` +
        `segments=${sent.segments} operator=${sent.operator} estimatedCostXaf=${sent.estimatedCostXaf}`,
    );

    if (WEBHOOK_APP_ID === undefined) {
      const created = await withDeadline(
        client.getMessage(sent.messageId),
        deadlineMs,
        "getMessage",
      );
      if (typeof created.appId !== "string" || created.appId === "") {
        throw new Error("the gateway's answer for the message names no appId");
      }
      learnedAppId(created.appId);
    }

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

    const decision = decide({ delivered, verifiedCount, eventCount: events.length });
    exitCode = decision.exitCode;

    if (decision.exitCode === 0) {
      console.log(
        `[demo-app] SUCCESS: messageId=${sent.messageId} reached delivered with ${verifiedCount} verified webhook(s) of ${events.length} received`,
      );
    } else {
      // decide()'s own "message never reached delivered" reason is
      // deliberately generic (no I/O, no `final` — see decision.ts's own
      // doc comment); this is the one place that adds the runtime detail
      // back in for the printed line, without decision.ts itself needing
      // to know about `Message`/`final` at all.
      const reasons = decision.reasons.map((reason) =>
        reason === "message never reached delivered"
          ? `${reason} (final observed state: ${final?.state ?? "timed out waiting"})`
          : reason,
      );
      console.error(`[demo-app] FAILURE: messageId=${sent.messageId} — ${reasons.join("; ")}`);
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
