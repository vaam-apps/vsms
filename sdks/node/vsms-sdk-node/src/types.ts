/**
 * Types describing vsms domain models and procedures.
 *
 * DESIGN DECISION: These types are deliberately hand-written rather than
 * generated via `cratestack generate-typescript`.
 *
 * The full schema is large (23 models, 18 enums, 18 procedures), mostly
 * concerning admin console operations and internal routing state. This SDK
 * is focused purely on the integrator experience: sending messages, checking
 * delivery, and previewing routing. Exposing the full CrateStack-generated
 * types would clutter the SDK with internal models that integrators cannot
 * access anyway. By hand-writing these types, we curate the public API
 * surface to exactly what the SDK exposes.
 */
export type Encoding = "gsm7" | "ucs2";
export type OperatorCode = "mtn" | "orange" | "camtel" | "nexttel" | "unknown";
export type MessageClass = "otp" | "transactional" | "notification" | "marketing";

/** What a provider's receipt said about a message. `in_flight` is progress, not an outcome. */
export type DeliveryOutcome =
  | "delivered"
  | "uncertain"
  | "failed"
  | "expired"
  | "rejected"
  | "in_flight"
  | "unknown";

export type MessageState =
  | "accepted"
  | "queued"
  | "routed"
  | "submitted"
  | "delivered"
  | "uncertain"
  | "undelivered"
  | "failed"
  | "expired"
  | "rejected"
  | "cancelled";

export interface PreviewInput {
  body: string;
  to?: string;
}

export interface PreviewResult {
  encoding: Encoding;
  segments: number;
  length: number;
  perSegment: number;
  offending: string[];
  suggestion?: string;
  operator: OperatorCode;
  normalizedTo?: string;
}

export interface SendMessageInput {
  to: string;
  body: string;
  senderId?: string;
  class?: MessageClass;
  clientRef?: string;
  /** ISO-8601 string */
  scheduledAt?: string;
  validityMinutes?: number;
}

export interface SendMessageResult {
  messageId: string;
  state: MessageState;
  encoding: Encoding;
  segments: number;
  operator: OperatorCode;
  estimatedCostXaf: string;
}

export interface Message {
  id: string;
  state: MessageState;
  to: string;
  body: string;
  senderId?: string;
  class: MessageClass;
  clientRef?: string;
  encoding: Encoding;
  segments: number;
  operator: OperatorCode;
  estimatedCostXaf: string;
  providerMessageRef?: string;
  createdAt: string;
  updatedAt: string;
  [key: string]: unknown;
}

/**
 * `lookupDeliveryStatus`'s query. Name exactly one of `messageId`, `clientRef` or
 * `msisdn`. A person names an app with `appId` (required with `clientRef`); an app
 * key is scoped to its own app and may leave it out.
 */
export interface DeliveryStatusQuery {
  messageId?: string;
  appId?: string;
  clientRef?: string;
  /** The whole number, e.g. `+237677123456`: matched exactly, never by prefix or suffix. */
  msisdn?: string;
  /** ISO-8601. Defaults to seven days before `until` (not applied to a `messageId`). */
  since?: string;
  /** ISO-8601. Defaults to now (not applied to a `messageId`). */
  until?: string;
  /** 1 to 50, default 10. */
  limit?: number;
}

/** A receipt as the lookup shows it: the provider's own payload is never included. */
export interface DeliveryReceiptSummary {
  id: string;
  providerId: string;
  outcome: DeliveryOutcome;
  rawStatus: string;
  errorCode: string | null;
  networkCode: OperatorCode;
  /** ISO-8601 */
  receivedAt: string;
  /** ISO-8601 */
  occurredAt: string | null;
}

/** One message's delivery state. It has no body and no full number. */
export interface DeliveryStatus {
  messageId: string;
  appId: string;
  /** The masked number (`+237*******56`); `null` once the message is purged. */
  recipient: string | null;
  operator: OperatorCode;
  senderIdValue: string;
  class: MessageClass;
  encoding: Encoding;
  segments: number;
  state: MessageState;
  attempts: number;
  /** ISO-8601 */
  acceptedAt: string;
  scheduledAt: string | null;
  submittedAt: string | null;
  finalizedAt: string | null;
  expiresAt: string;
  purged: boolean;
  lastReceipt: DeliveryReceiptSummary | null;
}

/** Newest first. `truncated` is true when more matched than `limit`. */
export interface DeliveryStatusResult {
  statuses: DeliveryStatus[];
  truncated: boolean;
}
