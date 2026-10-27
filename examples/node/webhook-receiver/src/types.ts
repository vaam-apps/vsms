/**
 * What this receiver reports about each request it handled. The event
 * itself is the SDK's `WebhookEvent`: `docs/architecture.md` section 8.4's
 * envelope plus the `eventId` that used to travel in `X-Sms-Event-Id` and
 * now rides inside the sealed body.
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
