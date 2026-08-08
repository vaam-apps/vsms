import type { Server } from "node:http";
import { setTimeout as sleep } from "node:timers/promises";
import express, { type Request, type Response } from "express";
import { verifySignature } from "./signature.ts";
import { WebhookStore } from "./store.ts";
import type { ProcessedResult, WebhookEnvelope } from "./types.ts";
import { AsyncWorkQueue } from "./work-queue.ts";

export interface ReceiverOptions {
  port: number;
  secret: string;
  prevSecret?: string | undefined;
  /** Simulates the real work a receiver does after ack (write to a DB,
   *  trigger a downstream side effect, ...). Kept deliberately visible in
   *  the logs so the "fast ack, work off the request path" claim is
   *  something you can watch happen, not just take on faith. */
  simulatedWorkMs?: number;
  onProcessed?: (result: ProcessedResult) => void;
}

export interface Receiver {
  store: WebhookStore;
  listen(): Promise<Server>;
}

/**
 * §8.4's envelope carries per-domain data (`data.messageId` for message.*
 * events) rather than a generic `aggregateId` field — this demo only drives
 * `message.*` events, so `messageId` is enough. A receiver that also
 * subscribes to `provider.*` / `sender_id.*` / `balance.*` /
 * `quota.*` events would need an equivalent extractor per domain; that's
 * out of scope for this example.
 */
function extractAggregateId(envelope: WebhookEnvelope): string {
  const messageId = envelope.data.messageId;
  if (typeof messageId === "string" && messageId.length > 0) return messageId;
  // Fall back to the envelope's own id rather than throwing — a receiver
  // should never 500 on a shape it doesn't fully recognise (see #150's own
  // scope: this is a reference example, not full envelope-schema coverage).
  return envelope.id;
}

export function createReceiver(options: ReceiverOptions): Receiver {
  const store = new WebhookStore();
  const queue = new AsyncWorkQueue();
  const simulatedWorkMs = options.simulatedWorkMs ?? 60;
  const secrets = [options.secret, options.prevSecret].filter((s): s is string => Boolean(s));

  const app = express();
  app.disable("x-powered-by");

  app.post(
    "/webhooks/vsms",
    // Raw bytes, not parsed JSON — the signature covers the exact bytes
    // vsms sent (sha256(body) in the signing string), and re-serialising a
    // parsed object is not guaranteed to reproduce them byte-for-byte.
    express.raw({ type: "application/json", limit: "1mb" }),
    (req: Request, res: Response) => {
      const receivedAt = Date.now();
      const rawBody = req.body as Buffer;
      const timestamp = req.header("x-sms-timestamp");
      const eventId = req.header("x-sms-event-id");
      const signatureHeader = req.header("x-sms-signature");

      const verdict = verifySignature({ rawBody, timestamp, eventId, signatureHeader, secrets });
      if (!verdict.ok) {
        options.onProcessed?.({ status: "rejected-signature", detail: verdict.reason });
        // 401: an auth failure, not a transient server problem. §8.5 treats
        // anything outside {2xx, 410} as retryable — a genuinely forged
        // request retrying forever is a cost we accept over guessing which
        // 4xx codes a not-yet-built sender (#41) would treat as terminal.
        res.status(401).json({ error: "invalid signature", reason: verdict.reason });
        return;
      }

      let envelope: WebhookEnvelope;
      try {
        envelope = JSON.parse(rawBody.toString("utf8")) as WebhookEnvelope;
      } catch {
        options.onProcessed?.({ status: "rejected-malformed", detail: "body was not valid JSON" });
        res.status(400).json({ error: "malformed JSON body" });
        return;
      }

      // ACK NOW. Everything after this point runs off the request path.
      res.status(202).json({ received: true });
      const ackedAt = Date.now();

      queue.push(async () => {
        const aggregateId = extractAggregateId(envelope);

        const isNewTuple = store.recordIfNew(aggregateId, envelope.type);
        if (!isNewTuple) {
          options.onProcessed?.({
            status: "accepted-duplicate",
            eventType: envelope.type,
            aggregateId,
            detail: `duplicate of an already-processed (aggregateId, eventType) tuple — skipped, no re-processing`,
          });
          return;
        }

        // Simulated slow work — e.g. a real DB write — proving it happens
        // strictly after the HTTP response already went out.
        await sleep(simulatedWorkMs);
        const processedAt = Date.now();

        const { applied, current } = store.applyOrdered(
          aggregateId,
          envelope.type,
          envelope.occurredAt,
        );
        if (!applied) {
          options.onProcessed?.({
            status: "accepted-out-of-order-ignored",
            eventType: envelope.type,
            aggregateId,
            detail:
              `arrived after a higher-precedence state ("${current.eventType}") already recorded for ` +
              `${aggregateId} — dedupe tuple kept, displayed state left unchanged ` +
              `(ack->processed in ${processedAt - ackedAt}ms, request total ${processedAt - receivedAt}ms)`,
          });
          return;
        }

        options.onProcessed?.({
          status: "accepted-new",
          eventType: envelope.type,
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
    listen(): Promise<Server> {
      return new Promise((resolve) => {
        const server = app.listen(options.port, () => resolve(server));
      });
    },
  };
}
