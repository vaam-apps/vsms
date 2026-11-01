// `POST /$procs/lookupDeliveryStatus` — the body-free "did this message
// arrive?" lookup (#472), for a person who holds `delivery:read` and may not
// read the message itself (the built-in `support` role).
//
// Types are transcribed from `schemas/vsms.cstack`'s `DeliveryStatusQuery`,
// `DeliveryStatus` and `DeliveryStatusResult`, with a nullable column as
// `field?: T | undefined`: the wire carries an explicit `null`, and `./json.ts`
// converts it before this module sees the body.
//
// # This is a read that is not free
//
// Every lookup that reaches the database writes a `DeliveryLookup` row, and a
// lookup by number or by client reference spends the caller's hourly budget.
// The console therefore calls this from a tRPC *mutation*, never a query (see
// `@vsms/api`'s `routers/delivery-status.ts`); nothing in this module caches,
// retries or repeats a call.
//
// # What the answer never carries
//
// No body, no body hash, no full number. `recipient` is the server's masked
// form (`+237*******56`) and is absent once the message is purged.

import type { Encoding, MessageClass, MessageState, OperatorCode } from "./client";
import type { DeliveryReceiptSummary } from "./messages";
import { postJson } from "./rest";

/** `DeliveryStatusQuery`. Exactly one of `messageId`, `clientRef` (with
 * `appId`) or `msisdn` names the message; the server refuses any other
 * combination with a 422. `since`/`until` are ISO instants. */
export interface DeliveryStatusQuery {
  messageId?: string | undefined;
  appId?: string | undefined;
  clientRef?: string | undefined;
  msisdn?: string | undefined;
  since?: string | undefined;
  until?: string | undefined;
  limit?: number | undefined;
}

/** `DeliveryStatus`: one message's delivery state, without its body. */
export interface DeliveryStatus {
  messageId: string;
  appId: string;
  /** Masked (`+237*******56`); absent once the message is purged. */
  recipient?: string | undefined;
  operator: OperatorCode;
  senderIdValue: string;
  class: MessageClass;
  encoding: Encoding;
  segments: number;
  state: MessageState;
  attempts: number;
  acceptedAt: string;
  scheduledAt?: string | undefined;
  submittedAt?: string | undefined;
  finalizedAt?: string | undefined;
  expiresAt: string;
  purged: boolean;
  lastReceipt?: DeliveryReceiptSummary | undefined;
}

/** `DeliveryStatusResult`: newest first; `truncated` when more matched than
 * the limit. */
export interface DeliveryStatusResult {
  statuses: DeliveryStatus[];
  truncated: boolean;
}

/** Drops a key whose value is `undefined`, so the body carries no key the
 * caller did not set (`JSON.stringify` would drop it anyway; this keeps the
 * intent visible and the argument object honest). */
function definedOnly(query: DeliveryStatusQuery): Record<string, unknown> {
  return Object.fromEntries(Object.entries(query).filter(([, value]) => value !== undefined));
}

/** Looks a message's delivery status up. Throws a `GatewayError`: 403 for a
 * missing `delivery:read` or an exhausted hourly budget (the message tells
 * them apart), 422 for an argument the server refuses. */
export async function lookupDeliveryStatus(
  query: DeliveryStatusQuery,
): Promise<DeliveryStatusResult> {
  return postJson<DeliveryStatusResult>(
    "/$procs/lookupDeliveryStatus",
    { args: definedOnly(query) },
    "lookupDeliveryStatus",
  );
}
