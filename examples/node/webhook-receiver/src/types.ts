import type { IncomingHttpHeaders } from "node:http";

/**
 * One delivery as it arrived: the raw body bytes and the request headers. A
 * `sealed` delivery is checked from the body alone, an `hmac` one from the
 * body and three headers, so the seam hands an opener both.
 */
export interface Delivery {
  body: Uint8Array;
  headers: IncomingHttpHeaders;
}

/**
 * The part of an opened event this receiver acts on, in either mode.
 *
 * - `sealed`: the SDK's `WebhookEvent` (`docs/architecture.md` section 8.4's
 *   envelope plus the `eventId`, `appId` and `endpointId` that ride inside the
 *   sealed body); `id` is `WebhookAttempt.id`.
 * - `hmac`: the original four-field envelope; `id` is the signed
 *   `X-Sms-Event-Id` header, the dedupe key that mode has always documented.
 *
 * Either way `id` is the key to dedupe on: a retry and an operator's replay
 * carry the same one.
 */
export interface ReceivedEvent {
  id: string;
  type: string;
  occurredAt: string;
  data: unknown;
}

/**
 * What this receiver reports about each request it handled.
 */
export type ProcessedStatus =
  | "accepted-new"
  | "accepted-duplicate"
  | "accepted-out-of-order-ignored"
  | "rejected-unverified"
  | "rejected-malformed"
  | "failed-backend";

export interface ProcessedResult {
  status: ProcessedStatus;
  eventType?: string;
  aggregateId?: string;
  detail: string;
}
