/**
 * Types describing vsms domain models and procedures.
 *
 * DESIGN DECISION: These types are deliberately hand-written rather than
 * generated via `cratestack generate-typescript`.
 *
 * The full schema is large (23 models, 13 enums, 7 procedures), mostly
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
