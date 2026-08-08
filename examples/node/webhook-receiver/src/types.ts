/**
 * The event envelope shape from `docs/architecture.md` §8.4's worked
 * example. This is the *design doc's* shape, not a shape ever observed from
 * a real vsms — outbound delivery (#38–#42) doesn't exist yet, so nothing
 * has ever actually sent one of these.
 */
export interface WebhookEnvelope {
  /** CrateStack's outbox `event_id` (§2.10's `sourceEventId`) — for tracing
   *  and duplicate diagnosis only. NOT the dedupe key; see store.ts. */
  id: string;
  /** e.g. "message.delivered" — see §8.4's event catalogue. */
  type: string;
  occurredAt: string;
  data: Record<string, unknown> & { messageId?: string };
}

export type ProcessedStatus =
  | "accepted-new"
  | "accepted-duplicate"
  | "accepted-out-of-order-ignored"
  | "rejected-signature"
  | "rejected-malformed";

export interface ProcessedResult {
  status: ProcessedStatus;
  eventType?: string;
  aggregateId?: string;
  detail: string;
}
