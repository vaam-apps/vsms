import "server-only";

// `GET /messages` and `GET /messages/{id}` — hand-written, same temporary
// seam as `client.ts` (see its module doc for why: T3/`packages/sms-client`
// is blocked on an upstream cratestack release). This file exists
// separately from `client.ts` because it talks to a genuinely different
// route shape — REST list/detail routes with a query grammar, not
// `POST /$procs/*` — not because of any package-boundary reason.
//
// # The query-grammar probe (T10's blocking prerequisite)
//
// `docs/architecture.md` §3.1 documents `?limit&offset&sort=-createdAt&
// where=...`, and §2.0 separately warns that `where`/`or` are "unwired in
// some generators." Neither claim was trusted — both were checked against
// a real, running `sms-gateway serve` (a scratch Postgres database seeded
// with real `App`/`Provider`/`Message` rows via `provisionAppClient`,
// real `client_credentials` + `private_key_jwt` token exchange, then
// `curl` with a real Bearer token). Findings, all reproduced live:
//
// 1. **`POST /$procs/listMessagesPage` is not usable.** `procedures.rs`
//    hard-codes it to `not_yet("listMessagesPage", "milestone 2")` —
//    confirmed over real HTTP, not just by reading the source: it returns
//    `500 INTERNAL_ERROR` for any input. This settles the "two candidate
//    paths" question in this task's brief: `GET /messages` /
//    `GET /messages/{id}` is the only reachable path today.
// 2. **`Accept: application/json` is mandatory.** Without it the gateway
//    tries to encode the response as `application/cbor` and 406s with "no
//    encoder configured" — there is no default JSON fallback.
//    `mkAcceptJsonHeaders` below always sends it.
// 3. **The response envelope** is `{ items, totalCount, pageInfo: { limit,
//    offset, hasNextPage, hasPreviousPage } }`. `totalCount` reflects
//    whatever `where`/bare-field filters were actually applied — it is
//    NOT the row's app-wide total.
// 4. **`limit`/`offset`/`sort=<field>` / `sort=-<field>` all work exactly
//    as documented.** `limit` is capped at 1000 server-side (`limit must
//    not exceed 1000`); requesting more is a 400, not a silent clamp.
//    `offset` must be `>= 0`.
// 5. **`fields=a,b,c` is real and is the PII control this project needs.**
//    A comma-separated projection narrows the returned columns, and an
//    unrecognised field name is a loud `400 VALIDATION_ERROR` ("unsupported
//    fields selection '<field>' for Message"), not a silent no-op. Verified
//    both that a narrow projection excludes `body`/`msisdn` when they are
//    not requested, and that requesting them explicitly still returns them
//    (i.e. the server enforces no confidentiality boundary here — the
//    projection is a bandwidth/PII-hygiene control this client must apply
//    itself, exactly as `message-stream.ts`'s module doc says).
// 6. **Bare `field=value` query params filter by exact-match equality, but
//    ONLY for non-null, non-enum scalar columns.** Confirmed working:
//    `id`, `appId`, `msisdn`, `msisdnHash`, `senderIdValue`, `bodyHash`,
//    `bodyLength`, `segments`, `attempts`, `maxAttempts`, `priority`,
//    `createdAt`, `updatedAt`, `expiresAt`, `version`. Confirmed NEVER
//    filterable, every time with `400 {"unsupported query filter '<field>'
//    for Message"}`: every enum column (`state`, `class`, `operator`,
//    `encoding`) regardless of value shape, and every nullable column
//    tried (`clientRef`, `stateReason`, `routeId`, `providerId`,
//    `providerMessageRef`, `idempotencyKey`, `leaseOwner`) regardless of
//    type. §2.0's warning was right to flag this area as unreliable.
// 7. **`where=key=value` is real recognised syntax** (a malformed
//    `where=state:failed` — colon instead of `=` — gets its own parse
//    error, "expected key=value") **but is gated by the identical
//    allowlist as bare params.** `where=state=failed` fails with the exact
//    same "unsupported query filter 'state'" as `?state=failed`. It buys
//    nothing beyond the bare-param form for this model.
// 8. **No comparison operators exist in the grammar at all** — `gte`/`lte`
//    in every shape tried (`createdAt[gte]=`, `createdAt.gte=`,
//    `createdAt_gte=`) is either silently mis-parsed as an unknown filter
//    key or rejected outright. Only exact equality is possible server-side.
// 9. **Row-level policy scoping cannot be bypassed by query params,**
//    confirmed live: passing `?appId=<a different app's id>` to `GET
//    /messages` returns an EMPTY list (not the other app's rows, not an
//    error) — the token's own `appId` wins regardless of what the caller
//    asks for. `GET /messages/{id}` for another app's message returns a
//    real `404`, not the data and not a `403`.
//
// # The consequence for this module's shape
//
// `state`, `clientRef` and date-range are exactly the filters T12 needs
// and exactly the ones finding 6 rules out — none of them are expressible
// server-side. `listMessages` below therefore fetches a single bounded,
// sorted window from `GET /messages` (`sort=-createdAt`, `limit` capped at
// the server's own max of 1000) and applies `state`/`clientRef`/date-range
// filtering, and the requested page's `limit`/`offset` slicing, **in this
// module, in Node** — never pretending the server did it. `truncated` on
// the result says plainly when the fetched window might not contain every
// match (more than 1000 messages exist for the app within the current
// sort order). This is an accepted, documented MVP limit, not a hidden
// one: a real fix needs either `listMessagesPage` actually implemented
// server-side, or CrateStack's REST filter grammar extended to cover enum
// columns — neither is this task's scope.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import type { Encoding, MessageClass, MessageState, OperatorCode } from "./client";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";
import { getMachineAccessToken, invalidateMachineAccessToken } from "./token";

/** The server's own enforced ceiling (`limit must not exceed 1000`) — also
 * used as the size of the bounded window `listMessages` fetches before
 * applying its own client-side filters (module doc, point 6/9 above). */
const MAX_SERVER_LIMIT = 1000;

/** The full row shape `GET /messages`/`GET /messages/{id}` can return.
 * Transcribed from a live response, field-for-field.
 *
 * **Correction, #221: this doc comment used to claim a nullable column is
 * "omitted from JSON when `null`... confirmed live" for every field here.**
 * That was checked live for exactly one field, `stateReason`, and never
 * true in general — #50 found a real, contradicting `"submittedAt": null`
 * in a genuine response body (`admin/app/messages/[id]/timeline.ts`'s own
 * doc has the full story), which is precisely what let a `!== undefined`
 * check through and rendered a bogus Unix-epoch timeline entry. The correct
 * statement is the opposite: sms-api always sends an explicit JSON `null`
 * for a nullable column with no value, never an omitted key.
 * `packages/gateway/src/json.ts` is now the one place this package converts
 * that `null` to this module's own `undefined`-only convention, applied to
 * every field below without this module needing its own `normalize*`
 * function — see that file's own module doc for the full mechanism. */
export interface MessageRecord {
  id: string;
  appId: string;
  clientRef?: string | undefined;
  idempotencyKey?: string | undefined;
  msisdn: string;
  msisdnHash: string;
  operator: OperatorCode;
  senderIdValue: string;
  class: MessageClass;
  priority: number;
  body?: string | undefined;
  bodyHash: string;
  bodyLength: number;
  encoding: Encoding;
  segments: number;
  state: MessageState;
  stateReason?: string | undefined;
  routeId?: string | undefined;
  providerId?: string | undefined;
  providerMessageRef?: string | undefined;
  providerMessageRefAlt?: string | undefined;
  attempts: number;
  maxAttempts: number;
  leaseOwner?: string | undefined;
  leaseUntil?: string | undefined;
  scheduledAt?: string | undefined;
  expiresAt: string;
  submittedAt?: string | undefined;
  finalizedAt?: string | undefined;
  /** `Decimal` on the wire — a string, never parsed to `number` (money-safety convention). */
  costXaf: string;
  version: number;
  createdAt: string;
  updatedAt: string;
}

/** The projection T12's list table actually renders — status, identity,
 * mono metadata, time (design doc §6.4) — deliberately excluding `body`.
 * `msisdn` IS included here: unlike the stream contract (`message-
 * stream.ts`), this is an authenticated, app-scoped, on-demand admin list
 * fetch, not a payload replicated to every open tab — the design doc's
 * own data-display rules (§7.1) assume the operator can see the MSISDN in
 * the messages table. */
const LIST_FIELDS = [
  "id",
  "appId",
  "clientRef",
  "msisdn",
  "operator",
  "senderIdValue",
  "class",
  "state",
  "stateReason",
  "encoding",
  "segments",
  "providerMessageRef",
  "version",
  "createdAt",
  "updatedAt",
  "submittedAt",
  "finalizedAt",
] as const;

export interface ListMessagesInput {
  state?: MessageState | undefined;
  clientRef?: string | undefined;
  /** ISO-8601, inclusive. Filtered client-side — see module doc point 8. */
  from?: string | undefined;
  /** ISO-8601, exclusive. Filtered client-side — see module doc point 8. */
  to?: string | undefined;
  limit?: number | undefined;
  offset?: number | undefined;
  sort?: "createdAt" | "-createdAt" | undefined;
}

export type MessageListItem = Pick<MessageRecord, (typeof LIST_FIELDS)[number]>;

export interface ListMessagesResult {
  items: MessageListItem[];
  /** Count after this module's own client-side filters, within the
   * fetched window — see `truncated`. */
  totalCount: number;
  hasNextPage: boolean;
  /** True when the fetched window (capped at `MAX_SERVER_LIMIT`) may not
   * contain every message matching the filter — module doc's "consequence"
   * section. The UI surfaces this rather than silently under-counting. */
  truncated: boolean;
}

const DEFAULT_LIST_LIMIT = 50;

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function messagesUrl(path: string, query: Record<string, string | number | undefined>): string {
  const url = new URL(path, env.SMS_API_URL);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

/** `GET` with a Bearer token, retrying once on an unexpected 401 — same
 * shape as `client.ts`'s `callProcedure`, duplicated rather than shared:
 * this module and `client.ts` are two temporary, independently-replaceable
 * halves of the same seam (REST list/detail vs. `$procs`), and each is
 * small enough that sharing a helper isn't worth coupling their futures
 * together before T3 replaces both anyway.
 *
 * `resolveToken`/`onUnauthorized` are injected rather than hardcoded — see
 * this file's own two call sites, [`getJson`] and [`getJsonAsMachine`], and
 * `message-stream.ts`'s module doc for why `listMessagesForStream` must
 * never resolve the signed-in human's own token (#211): it is driven by
 * `MessageStreamHub`'s process-wide `setInterval` poll, not by any single
 * request, so it has no one human to act as, and — the sharper reason —
 * `AsyncLocalStorage` context set up by whichever request happens to first
 * trigger `hub.start()` would otherwise leak into every later poll tick,
 * including ticks serving *other* operators' own open tabs. */
async function getJsonWith<T>(
  resolveToken: () => Promise<string>,
  onUnauthorized: () => void,
  path: string,
  query: Record<string, string | number | undefined>,
  routeLabel: string,
): Promise<T | null> {
  const url = messagesUrl(path, query);

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveToken();
    return undiciFetch(url, {
      method: "GET",
      headers: {
        accept: "application/json",
        authorization: `Bearer ${token}`,
      },
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    onUnauthorized();
    response = await attempt();
  }

  if (response.status === 404) return null;

  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, routeLabel);
  }
  return parsed as T;
}

/** The signed-in operator's own credential — every ordinary admin-console
 * read in this file (`listMessages`, `getMessageById`) goes through this.
 * `Message.list`/`.detail`'s own `@@allow` admits `auth().kind == "user"`
 * unconditionally (schema.cstack), so this has been reachable by any
 * authenticated human since before #211 — #211 is what actually forwards
 * one. */
function getJson<T>(
  path: string,
  query: Record<string, string | number | undefined>,
  routeLabel: string,
): Promise<T | null> {
  return getJsonWith(
    resolveUpstreamAccessToken,
    invalidateUpstreamAccessToken,
    path,
    query,
    routeLabel,
  );
}

/** The console's own machine credential, explicitly — see this function's
 * one caller, `listMessagesForStream`, and this file's own doc on
 * `getJsonWith` for why. */
function getJsonAsMachine<T>(
  path: string,
  query: Record<string, string | number | undefined>,
  routeLabel: string,
): Promise<T | null> {
  return getJsonWith(getMachineAccessToken, invalidateMachineAccessToken, path, query, routeLabel);
}

interface MessagesPage {
  items: MessageRecord[];
  totalCount: number;
  pageInfo: {
    limit: number;
    offset: number | null;
    hasNextPage: boolean;
    hasPreviousPage: boolean;
  };
}

function withinRange(createdAt: string, from: string | undefined, to: string | undefined): boolean {
  if (from !== undefined && createdAt < from) return false;
  if (to !== undefined && createdAt >= to) return false;
  return true;
}

function pickListFields(record: MessageRecord): MessageListItem {
  const out = {} as MessageListItem;
  for (const field of LIST_FIELDS) {
    // biome-ignore lint/suspicious/noExplicitAny: narrowing a mapped-tuple key back to its own field
    (out as any)[field] = record[field];
  }
  return out;
}

/**
 * `GET /messages`, windowed and filtered per this module's doc. Always
 * scoped to the calling app client's own app — enforced server-side by row
 * policy (module doc point 9), not by any parameter this function sends.
 */
export async function listMessages(input: ListMessagesInput = {}): Promise<ListMessagesResult> {
  const limit = input.limit ?? DEFAULT_LIST_LIMIT;
  const offset = input.offset ?? 0;
  const sort = input.sort ?? "-createdAt";

  const page = await getJson<MessagesPage>(
    "/messages",
    { limit: MAX_SERVER_LIMIT, sort, fields: LIST_FIELDS.join(",") },
    "listMessages",
  );
  const window = page?.items ?? [];

  const filtered = window.filter((record) => {
    if (input.state !== undefined && record.state !== input.state) return false;
    if (input.clientRef !== undefined && record.clientRef !== input.clientRef) return false;
    if (!withinRange(record.createdAt, input.from, input.to)) return false;
    return true;
  });

  const pageSlice = filtered.slice(offset, offset + limit);

  return {
    items: pageSlice.map(pickListFields),
    totalCount: filtered.length,
    hasNextPage: offset + limit < filtered.length,
    truncated: window.length >= MAX_SERVER_LIMIT,
  };
}

/** `GET /messages/{id}`. Returns `null` on a 404 — which, per module doc
 * point 9, is also what a real cross-app id (one that exists but belongs
 * to a different app) returns. Callers cannot distinguish "never existed"
 * from "not yours" — by design, matching the row-policy's own behaviour. */
export async function getMessageById(id: string): Promise<MessageRecord | null> {
  return getJson<MessageRecord>(`/messages/${encodeURIComponent(id)}`, {}, "getMessageById");
}

// --- #50: the message detail timeline's own receipt evidence. -----------
//
// `POST /$procs/listMessageReceipts`, not `GET /delivery_receipts` — see
// `schema/schema.cstack`'s own comment on the procedure declaration and
// `crates/sms-api/src/procedures.rs::message_receipts`'s doc for why:
// `DeliveryReceipt`'s own REST policy never admits this console's `"app"`
// -kind credential, so the procedure is the seam, gated by `@authorize`
// against the same `Message` ownership check `getMessageById` already
// relies on. A cross-app `messageId` denies with `Forbidden` at the
// framework layer before this procedure's own body ever runs — mapped to
// the same shape every other gateway error goes through
// (`mapGatewayError`), not specially handled here.
//
// This function deliberately returns the raw receipts, nothing more — it
// does NOT attempt to reconstruct a full state history from them. See
// `admin/app/messages/[id]/timeline.ts`'s own module doc for why: a
// `DeliveryReceipt` row is evidence a provider callback arrived and what
// it said, not proof of what state the message moved through, and
// `next_state`'s own outcome-to-transition mapping (`crates/sms-api/src/
// dlr.rs`) isn't invertible after the fact — a `failed` outcome can drive
// either `-> undelivered` or `-> failed` depending on the message's state
// at the moment the DLR was ingested, which this row alone does not
// record. Guessing would be exactly the "confident chronology it cannot
// prove" #50 explicitly forbids.

export type DeliveryOutcome =
  | "delivered"
  | "uncertain"
  | "failed"
  | "expired"
  | "rejected"
  | "unknown";

/** Transcribed from `schema.cstack`'s `DeliveryReceiptSummary` — the
 * console-facing projection of `DeliveryReceipt`, deliberately omitting
 * `rawPayload` (the provider's raw wire body never reaches this console). */
export interface DeliveryReceiptSummary {
  id: string;
  providerId: string;
  outcome: DeliveryOutcome;
  rawStatus: string;
  errorCode?: string | undefined;
  networkCode: OperatorCode;
  receivedAt: string;
  occurredAt?: string | undefined;
}

export interface MessageReceiptsResult {
  receipts: DeliveryReceiptSummary[];
}

function procedureUrl(procedure: string): string {
  return new URL(`/$procs/${procedure}`, env.SMS_API_URL).toString();
}

/**
 * `POST /$procs/listMessageReceipts` — every `DeliveryReceipt` row this
 * system has for one message, oldest first. Never throws on "no receipts
 * yet" (an empty array is a completely normal outcome — see this file's
 * own module doc on why a message can reach a terminal state with zero
 * receipts, e.g. an `Indeterminate` submit landing directly in
 * `uncertain`); it throws the same `GatewayError` shape every other call
 * in this package does for a genuine transport/auth/policy failure.
 */
export async function listMessageReceipts(messageId: string): Promise<MessageReceiptsResult> {
  const url = procedureUrl("listMessageReceipts");

  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json",
        authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({ args: { messageId } }),
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    invalidateUpstreamAccessToken();
    response = await attempt();
  }

  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listMessageReceipts");
  }
  return parsed as MessageReceiptsResult;
}

// --- The stream hub's own, deliberately narrower, upstream fetch. -------
//
// `message-stream.ts`'s module doc explains why this is a SEPARATE
// function from `listMessages` rather than a thin wrapper over it:
// `listMessages` requests `LIST_FIELDS`, which includes `msisdn` — correct
// for an authenticated, on-demand admin list, and exactly what the PII
// control in `message-stream.ts` must never let onto the wire that reaches
// every open browser tab. `STREAM_FIELDS` is the fixed browser contract's
// own field set, nothing more: this function's `fields=` request to
// sms-api never includes `body` or `msisdn`, so there is no projection
// step downstream that could get skipped or miscoded — the data simply
// never arrives in this process for a stream poll.

const STREAM_FIELDS = [
  "id",
  "appId",
  "state",
  "stateReason",
  "operator",
  "segments",
  "version",
  "updatedAt",
  "providerMessageRef",
] as const;

export type StreamCandidate = Pick<MessageRecord, (typeof STREAM_FIELDS)[number]>;

/**
 * The most recently *changed* messages — sorted by `updatedAt` descending
 * (not `createdAt`: the state-machine trigger bumps `updatedAt` on every
 * transition, `createdAt` never changes, so this is the column that
 * actually orders "what moved most recently"). `windowSize` bounds the
 * request the same way `listMessages` bounds its own — a fixed recent
 * slice, not the whole table.
 */
export async function listMessagesForStream(windowSize: number): Promise<StreamCandidate[]> {
  const page = await getJsonAsMachine<{ items: StreamCandidate[] }>(
    "/messages",
    { limit: windowSize, sort: "-updatedAt", fields: STREAM_FIELDS.join(",") },
    "listMessagesForStream",
  );
  return page?.items ?? [];
}
