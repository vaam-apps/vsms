import "server-only";

// The two calls the composer (T13) needs: `previewMessage` and
// `sendMessage`. Hand-written, temporarily.
//
// **This is a temporary seam by design, not the final shape.** T3 will
// generate `packages/sms-client` from `schema/schema.cstack` once an
// upstream `Decimal` fix ships (cratestack#456, merged, release pending)
// — at which point this file's two functions get replaced by calls into
// the generated client, and every other file in this package
// (`token.ts`, `dispatcher.ts`, `errors.ts`) stays as-is. `@vsms/gateway`
// exists specifically so that swap touches this one package: nothing
// outside `@vsms/gateway` imports `undici` or knows the routes are
// `POST /$procs/...` rather than whatever shape the generated client
// prefers. `packages/api/src/routers/compose.ts` only ever imports
// `previewMessage`/`sendMessage` by name from here.
//
// Routes are served at `/`, not `/api` and not `/v1` — verified against a
// running `sms-gateway routes` (102 routes); `docs/architecture.md` §3.1's
// `/v1` prefix does not exist in this deployment. The procedure routes
// are exactly `POST /$procs/previewMessage` and `POST /$procs/sendMessage`
// (`schema/schema.cstack`'s `procedure previewMessage` / `mutation
// procedure sendMessage`).
//
// Types below are hand-transcribed from `schema/schema.cstack`'s
// `PreviewInput`/`PreviewResult`/`SendMessageInput`/`SendMessageResult`
// and the `Encoding`/`OperatorCode`/`MessageClass`/`MessageState` enums —
// faithfully, including `offending: string[]` being an array of the
// offending *characters*, not byte/codepoint offsets (T13 does the
// highlighting against that).

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { getMachineAccessToken, invalidateMachineAccessToken } from "./token";

export type Encoding = "gsm7" | "ucs2";
export type OperatorCode = "mtn" | "orange" | "camtel" | "nexttel" | "unknown";
export type MessageClass = "otp" | "transactional" | "notification" | "marketing";

/** All eleven states `messages_state_enum_check` can produce (AGENTS.md's
 * design-system note: "Eleven states, not ten" — `rejected` is real and
 * terminal, reachable via `accepted -> rejected` when no active provider
 * exists). */
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

// `field?: T | undefined` rather than plain `field?: T` throughout this
// file: `tsconfig.base.json`'s `exactOptionalPropertyTypes` (§3 of the
// architecture plan warns this causes "assignability friction on every
// field") distinguishes "key absent" from "key present with value
// `undefined`" — and a zod `.optional()`-parsed object (see
// `packages/api/src/routers/compose.ts`) is typed the second way. Writing
// the explicit union here, once, is cheaper than a stripping helper at
// every call site.

export interface PreviewInput {
  body: string;
  to?: string | undefined;
}

export interface PreviewResult {
  encoding: Encoding;
  segments: number;
  length: number;
  perSegment: number;
  /** Offending *characters*, not offsets — see module doc. */
  offending: string[];
  suggestion?: string | undefined;
  operator: OperatorCode;
  normalizedTo?: string | undefined;
}

export interface SendMessageInput {
  to: string;
  body: string;
  senderId?: string | undefined;
  class?: MessageClass | undefined;
  clientRef?: string | undefined;
  /** ISO-8601. */
  scheduledAt?: string | undefined;
  validityMinutes?: number | undefined;
}

export interface SendMessageResult {
  messageId: string;
  state: MessageState;
  encoding: Encoding;
  segments: number;
  operator: OperatorCode;
  /** `Decimal` on the wire — kept as a string, never parsed to `number`,
   * per this project's money-safety convention (never floating point for
   * minor units / currency amounts). */
  estimatedCostXaf: string;
}

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function procedureUrl(procedure: string): string {
  return new URL(`/$procs/${procedure}`, env.SMS_API_URL).toString();
}

/**
 * `POST /$procs/{procedure}` with a Bearer token, retrying exactly once —
 * with a freshly minted token — on an unexpected 401. `getMachineAccessToken`'s
 * own `exp - 60s` cache margin should make a 401 unreachable in normal
 * operation; the retry exists for the one case that margin can't cover, a
 * signing-key rotation invalidating the cached token mid-window.
 *
 * **Deliberately the console's machine credential, never the signed-in
 * human's own token (#211).** `crates/sms-api/src/procedures.rs::
 * Procedures::caller_client_id` — the function both `previewMessage` and
 * `sendMessage` call to resolve which `App` a send belongs to — hard-rejects
 * any caller whose `kind` isn't `"app"`: "sendMessage currently requires a
 * machine (client_credentials) caller — deriving an App for a human caller
 * has no design yet." Forwarding a human token here wouldn't merely be the
 * wrong credential, it would turn every composer send into a guaranteed
 * `Validation` error. `./request-credential.ts`'s own module doc names this
 * as one of the two call sites that must keep using `getMachineAccessToken`
 * explicitly rather than `resolveUpstreamAccessToken`.
 */
async function callProcedure<TArgs extends object, TResult>(
  procedure: string,
  args: TArgs,
): Promise<TResult> {
  const url = procedureUrl(procedure);
  const body = JSON.stringify({ args });

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await getMachineAccessToken();
    return undiciFetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json",
        authorization: `Bearer ${token}`,
      },
      body,
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    invalidateMachineAccessToken();
    response = await attempt();
  }

  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, procedure);
  }
  return parsed as TResult;
}

export async function previewMessage(input: PreviewInput): Promise<PreviewResult> {
  return callProcedure<PreviewInput, PreviewResult>("previewMessage", input);
}

export async function sendMessage(input: SendMessageInput): Promise<SendMessageResult> {
  return callProcedure<SendMessageInput, SendMessageResult>("sendMessage", input);
}
