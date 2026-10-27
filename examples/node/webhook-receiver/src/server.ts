import type { Server } from "node:http";
import { setTimeout as sleep } from "node:timers/promises";
import type { WebhookEvent } from "@vaam-apps/vsms-node";
import { SdkError } from "@vaam-apps/vsms-node";
import express, { type Request, type Response } from "express";
import { WebhookStore } from "./store.ts";
import type { ProcessedResult } from "./types.ts";
import { AsyncWorkQueue } from "./work-queue.ts";

/**
 * What the receiver needs from `@vaam-apps/vsms-node`'s `WebhookOpener`: the
 * body of one delivery in, the verified event out, or an `SdkError`. A seam so
 * the tests can hand the handler events (to drive an out-of-order arrival)
 * without sealing one; the shipped entry points pass the real opener.
 */
export interface EventOpener {
  open(body: Uint8Array): Promise<WebhookEvent>;
}

export interface ReceiverOptions {
  port: number;
  /** Opens a delivery: verifies it was sealed by the gateway, for this receiver. */
  opener: EventOpener;
  /**
   * Simulates the real work a receiver does after ack (write to a DB,
   * trigger a downstream side effect, ...). Kept deliberately visible in
   * the logs so the "fast ack, work off the request path" claim is
   * something you can watch happen, not just take on faith.
   */
  simulatedWorkMs?: number;
  onProcessed?: (result: ProcessedResult) => void;
}

export interface Receiver {
  store: WebhookStore;
  /**
   * The underlying Express app, exposed so a caller can mount additional
   * routes on the same port before calling `listen()` — e.g.
   * `gate-receiver.ts`'s `/__test__/results` diagnostic endpoint for
   * `#44`'s live gate.
   */
  app: express.Express;
  listen(): Promise<Server>;
}

/**
 * §8.4's envelope carries per-domain data (`data.messageId` for message.*
 * events) rather than a generic `aggregateId` field — this example only
 * handles `message.*` events, so `messageId` is enough. A receiver that also
 * subscribes to `provider.*` / `sender_id.*` / `balance.*` / `quota.*`
 * events would need an equivalent extractor per domain.
 */
function extractAggregateId(event: WebhookEvent): string {
  const data = event.data as { messageId?: unknown } | null;
  const messageId = data?.messageId;
  if (typeof messageId === "string" && messageId.length > 0) return messageId;
  // Fall back to the envelope's own id rather than throwing — a receiver
  // should never 500 on a shape it doesn't fully recognise.
  return event.id;
}

export function createReceiver(options: ReceiverOptions): Receiver {
  const store = new WebhookStore();
  const queue = new AsyncWorkQueue();
  const simulatedWorkMs = options.simulatedWorkMs ?? 60;

  const app = express();
  app.disable("x-powered-by");

  app.post(
    "/webhooks/vsms",
    // Raw bytes, not parsed JSON — the body is a COSE_Sign1 message
    // (`application/cose`), and the opener verifies exactly the bytes vsms
    // sent. Any content type: the type is not what is trusted.
    express.raw({ type: () => true, limit: "1mb" }),
    async (req: Request, res: Response) => {
      const receivedAt = Date.now();
      const rawBody = req.body instanceof Buffer ? req.body : Buffer.alloc(0);

      let event: WebhookEvent;
      try {
        event = await options.opener.open(rawBody);
      } catch (err) {
        const kind = err instanceof SdkError ? err.kind : "backend";
        const detail = err instanceof Error ? err.message : String(err);
        if (kind === "unverified") {
          options.onProcessed?.({ status: "rejected-unverified", detail });
          // 401: one answer for a forgery, a delivery addressed to another audience
          // or made for another app, a replay and a stale timestamp, exactly as the
          // opener gives it.
          // §8.5 treats anything outside {2xx, 410} as retryable, and a retry
          // is sealed afresh, so a delivery that failed on the clock heals.
          res.status(401).json({ error: "unverified" });
        } else if (kind === "decode" || kind === "unreadable") {
          options.onProcessed?.({ status: "rejected-malformed", detail });
          res.status(400).json({ error: "malformed delivery" });
        } else {
          options.onProcessed?.({ status: "failed-backend", detail });
          res.status(500).json({ error: "could not check the delivery" });
        }
        return;
      }

      // ACK NOW. Everything after this point runs off the request path.
      res.status(202).json({ received: true });
      const ackedAt = Date.now();

      queue.push(async () => {
        const aggregateId = extractAggregateId(event);

        // PRIMARY check: dedupe on the event's `id`. The (aggregateId,
        // eventType) tuple is a SECONDARY, defensive check — see store.ts's
        // own doc comment for why it's kept.
        const duplicateReason = store.checkDuplicate(event.id, aggregateId, event.type);
        if (duplicateReason) {
          options.onProcessed?.({
            status: "accepted-duplicate",
            eventType: event.type,
            aggregateId,
            detail:
              duplicateReason === "event-id"
                ? `duplicate event id "${event.id}" — already processed, skipped (a retry is a new sealed request carrying the same id)`
                : `distinct event id but duplicate (aggregateId, eventType) tuple — skipped ` +
                  `(secondary, defensive check beyond the documented contract)`,
          });
          return;
        }
        store.recordSeen(event.id, aggregateId, event.type);

        // Simulated slow work — e.g. a real DB write — proving it happens
        // strictly after the HTTP response already went out.
        await sleep(simulatedWorkMs);
        const processedAt = Date.now();

        const { applied, current } = store.applyOrdered(aggregateId, event.type, event.occurredAt);
        if (!applied) {
          options.onProcessed?.({
            status: "accepted-out-of-order-ignored",
            eventType: event.type,
            aggregateId,
            detail:
              `arrived after a higher-precedence state ("${current.eventType}") already recorded for ` +
              `${aggregateId} — dedupe keys recorded (won't reprocess a retry of this event), ` +
              `displayed state left unchanged ` +
              `(ack->processed in ${processedAt - ackedAt}ms, request total ${processedAt - receivedAt}ms)`,
          });
          return;
        }

        options.onProcessed?.({
          status: "accepted-new",
          eventType: event.type,
          aggregateId,
          detail:
            `processed; ${aggregateId} now shows state "${current.eventType}" ` +
            `(ack->processed in ${processedAt - ackedAt}ms, request total ${processedAt - receivedAt}ms)`,
        });
      });
    },
  );

  app.get("/healthz", (_req: Request, res: Response) => {
    res.status(200).json({ ok: true, aggregatesTracked: store.aggregateCount });
  });

  return {
    store,
    app,
    listen(): Promise<Server> {
      return new Promise((resolve) => {
        const server = app.listen(options.port, () => resolve(server));
      });
    },
  };
}
