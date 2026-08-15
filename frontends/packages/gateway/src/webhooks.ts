import "server-only";

// `GET /webhook_endpoints`, `GET /webhook_attempts`, `POST`/`PATCH`/`DELETE
// /webhook_endpoints/{id}`, and the two procedures `rotateWebhookSecret`/
// `replayWebhookAttempt` (#41/#43, already real and tested server-side) —
// #55's Webhooks screen. Same temporary hand-written seam as
// `providers.ts`/`routes.ts`/`senders.ts` (see `client.ts`'s module doc for
// why).
//
// # `WebhookEndpoint.secret`/`prevSecret` — displayed, not hidden, and here
// is why that is the honest choice rather than the cautious-looking one
//
// AGENTS.md's own trap, verbatim: "`@sensitive` does NOT redact it from an
// API response — it only affects audit snapshots." `WebhookEndpoint.read`'s
// `@@allow` (`schema.cstack`, narrowed by #187) is `hasRole('owner') ||
// hasRole('admin') || hasRole('developer') || hasRole('system')` — a real
// GET by any of those three human roles genuinely returns the live
// plaintext `secret`/`prevSecret` columns, every time, not once at creation
// the way (say) `provisionAppClient`'s private key or `provision-user`'s
// generated password are. There is no server-side mechanism this console
// could add to make it a true one-time secret without changing
// `WebhookEndpoint`'s own schema (storing a hash instead, which would break
// `sms_webhook::verify`'s own "try every candidate secret" design — it needs
// the plaintext to HMAC against). So this module does not pretend otherwise:
// it returns the real value on every read, and `webhooks-screen.tsx` masks
// it *in the UI* (a display discipline against shoulder-surfing and screen
// shares, with an explicit "Reveal" toggle) rather than withholding data
// that already crossed the wire to an authorized, authenticated browser
// session — withholding it here would be security theatre, not a boundary.
//
// # Create needs a caller-supplied secret — `WebhookEndpoint.secret` has no
// `@default`
//
// Unlike `OauthSigningKey`/`AppClient`, nothing generates this endpoint's
// initial secret server-side — `createWebhookEndpoint` below does it here,
// with `crypto.randomBytes(32)`, in the exact wire format
// `sms_webhook::generate_secret()` produces (`whsec_<64 lowercase hex
// chars>`, verified against that function's own doc: 32 bytes of secure
// randomness, hex-encoded). This is the one place in this file that
// generates a secret rather than sending one the operator typed — the
// created row's `secret` is the same value this function already has, so
// `webhooks-screen.tsx`'s create dialog can show a "copy it now" banner
// exactly once, the same UX beat `provision-user`'s printed-once password
// has, even though (per the section above) the value is not actually gone
// from the API afterward — it is simply the *newest*, least-likely-to-need-
// re-copying moment to show it.
//
// # `eventTypes` — space-sentinel packed, not a JSON array on the wire
//
// `docs/architecture.md` §2.0's own "delimited strings" rule: `String[]`
// panics the server macro, so `eventTypes` is a `String` column packed as
// `" message.accepted message.delivered "` (leading/trailing spaces,
// `sms_core::pack`'s own format) — `dlr.rs`'s own
// `webhook_endpoint::eventTypes().contains(sms_core::needle(event_type))`
// match confirms the exact shape a real subscriber match depends on.
// [`packEventTypes`]/[`unpackEventTypes`] are this module's own faithful
// port, matching `sms_core::pack`/`unpack` byte-for-byte rather than
// guessing a shape that merely looks plausible.
//
// # Writes are real, and reachable, as of #211 — same shape `providers.ts`/
// `senders.ts` already document
//
// `WebhookEndpoint.create`/`.update`/`.delete` admit no `auth().kind ==
// "app"` clause — `owner`/`admin`/`developer` (create/update) or
// `owner`/`admin` (delete) only. Every write here resolves its Bearer token
// via `resolveUpstreamAccessToken()`, gated further at Layer 2 by
// `router::SENDER_AND_WEBHOOK_WRITE_ROUTES` (`webhook:manage`, granted to
// `owner`/`admin`/`developer` per §5.2's seeded roles). `rotateWebhookSecret`/
// `replayWebhookAttempt` (procedures, not REST) carry their own
// `require_permission(ctx, "webhook:manage")` server-side (#193/#43) —
// nothing new to gate here, this module just calls them.

import { randomBytes } from "node:crypto";
import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";
import { deleteResource, fetchWithEtag, updateWithIfMatch, type WithEtag } from "./rest";

/** §8.4's own catalogue, verbatim — the only seven event types any
 * subscriber in this deployment ever emits (`webhooks.rs`'s own
 * `message_event_type`; `queued`/`routed`/`undelivered`/`rejected`
 * deliberately produce none). Not `Message`'s eleven states — a fixed,
 * narrower list, which is why this isn't imported from `client.ts`'s own
 * `MessageState`. */
export const WEBHOOK_EVENT_TYPES = [
  "message.accepted",
  "message.submitted",
  "message.delivered",
  "message.failed",
  "message.expired",
  "message.uncertain",
  "message.cancelled",
] as const;

export type WebhookEventType = (typeof WEBHOOK_EVENT_TYPES)[number];

/** `attempt_state_transitions` (§2.10), verbatim — see `schema.cstack`'s
 * `AttemptState` enum. */
export type AttemptState = "pending" | "delivering" | "succeeded" | "failed" | "dead";

export const ATTEMPT_STATES: readonly AttemptState[] = [
  "pending",
  "delivering",
  "succeeded",
  "failed",
  "dead",
];

/** `sms_core::pack`'s own algorithm, ported field-for-field (`crates/
 * sms-core/src/lib.rs::pack`): start from a single separator (`EMPTY`,
 * `" "`), then for each value push the value followed by one more
 * separator. `pack(["a","b"]) == " a b "`; `pack([]) == " "` — one space,
 * not two. Getting this wrong for the empty case is the actual trap: a
 * naive `` ` ${types.join(" ")} ` `` produces `"  "` (two spaces) for an
 * empty selection, not `" "` — cosmetically identical after a `.trim()`,
 * genuinely different once a real membership check
 * (`webhook_endpoint::eventTypes().contains(sms_core::needle(...))`, `dlr.rs`)
 * runs against it, and different from what the same field's own `SET
 * DEFAULT ' '` (`0002_bootstrap`) already writes for a fresh row nothing
 * has touched. Exported for [`webhooks.test.ts`]'s own round-trip and
 * empty-case assertions. */
export function packEventTypes(types: readonly string[]): string {
  let packed = " ";
  for (const type of types) {
    packed += `${type} `;
  }
  return packed;
}

/** The inverse — `sms_core::unpack`'s own `split_whitespace` behaviour,
 * ported: tolerant of the DB's own stored leading/trailing spaces, of
 * repeated internal whitespace, and of an entirely empty (`" "`) column. */
export function unpackEventTypes(packed: string): WebhookEventType[] {
  return packed
    .split(/\s+/)
    .map((s) => s.trim())
    .filter((s): s is WebhookEventType => s.length > 0);
}

export interface WebhookEndpointRecord {
  id: string;
  appId: string;
  url: string;
  eventTypes: WebhookEventType[];
  /** The live plaintext HMAC secret — see this module's own doc for why
   * this is the real value, not a placeholder, and why that's correct. */
  secret: string;
  /** Set only while an overlap window is open (#41/#59: `rotateWebhookSecret`
   * moves the pre-rotation `secret` here). `sms_webhook::verify` accepts a
   * signature made with either this or `secret` for exactly this reason —
   * a receiver that hasn't picked up the new value yet keeps working. */
  prevSecret?: string | undefined;
  /** When the *current* `secret` was minted — `undefined` for an endpoint
   * that has never been rotated since creation. */
  secretRotatedAt?: string | undefined;
  maskRecipient: boolean;
  active: boolean;
  maxAttempts: number;
  circuitOpenUntil?: string | undefined;
  consecutiveFailures: number;
  version: number;
  createdAt: string;
  updatedAt: string;
}

export interface WebhookAttemptRecord {
  id: string;
  endpointId: string;
  sourceEventId: string;
  aggregateId: string;
  eventType: string;
  /** The `data` object §8.4 defines, as a JSON string — see `hooks.rs`'s
   * own doc: this is the `data` half only, not the full signed envelope
   * (`{id, type, occurredAt, data}`), which `hooks` builds at delivery time
   * and never persists separately. */
  payload: string;
  state: AttemptState;
  attempts: number;
  leaseOwner?: string | undefined;
  leaseUntil?: string | undefined;
  nextAttemptAt?: string | undefined;
  lastStatusCode?: number | undefined;
  lastError?: string | undefined;
  lastAttemptAt?: string | undefined;
  deliveredAt?: string | undefined;
  version: number;
}

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function gatewayUrl(path: string, query: Record<string, string | number | undefined> = {}): string {
  const url = new URL(path, env.SMS_API_URL);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

interface WireEndpoint extends Omit<WebhookEndpointRecord, "eventTypes"> {
  eventTypes: string;
}

/** The one transformation this module still needs beyond #221's generic
 * `null` -> `undefined` seam (`./json.ts`, applied automatically to every
 * response this module parses): `eventTypes` is a sentinel-packed `String`
 * on the wire, not the `WebhookEventType[]` this module's own types declare
 * — [`unpackEventTypes`] is model-specific shape logic, not null-handling,
 * so it stays here rather than folding into the generic seam. */
function normalizeEndpoint(row: WireEndpoint): WebhookEndpointRecord {
  return { ...row, eventTypes: unpackEventTypes(row.eventTypes) };
}

async function authedRequest(
  url: string,
  init: { method: "GET" | "POST" | "PATCH" | "DELETE"; body?: string; ifMatch?: string },
): Promise<UndiciResponse> {
  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: init.method,
      headers: {
        accept: "application/json",
        ...(init.body !== undefined ? { "content-type": "application/json" } : {}),
        ...(init.ifMatch !== undefined ? { "if-match": init.ifMatch } : {}),
        authorization: `Bearer ${token}`,
      },
      ...(init.body !== undefined ? { body: init.body } : {}),
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    invalidateUpstreamAccessToken();
    response = await attempt();
  }
  return response;
}

/** `GET /webhook_endpoints` — every endpoint this deployment has, across
 * every app (no `appId` filter server-side; `WebhookEndpoint` carries one,
 * but this console-wide diagnostics screen doesn't scope by it, the same
 * "whole-system, not per-app" shape `jobs.ts`'s own module doc documents
 * for `Job`). */
export async function listWebhookEndpoints(): Promise<WebhookEndpointRecord[]> {
  const response = await authedRequest(gatewayUrl("/webhook_endpoints"), { method: "GET" });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listWebhookEndpoints");
  }
  return (parsed as WireEndpoint[]).map(normalizeEndpoint);
}

export async function getWebhookEndpointById(
  id: string,
): Promise<WithEtag<WebhookEndpointRecord> | null> {
  const result = await fetchWithEtag<WireEndpoint>(
    `/webhook_endpoints/${encodeURIComponent(id)}`,
    "getWebhookEndpoint",
  );
  return result === null ? null : { ...result, data: normalizeEndpoint(result.data) };
}

export interface CreateWebhookEndpointFields {
  appId: string;
  url: string;
  eventTypes: WebhookEventType[];
  maskRecipient: boolean;
  maxAttempts: number;
}

export interface CreatedWebhookEndpoint {
  endpoint: WebhookEndpointRecord;
  /** The freshly generated plaintext secret — identical to
   * `endpoint.secret`, surfaced separately so the create dialog can show a
   * "copy it now" banner without every other caller of this type needing
   * to know that's what a fresh creation means. See module doc. */
  secret: string;
}

/** `POST /webhook_endpoints`. Generates the initial secret here — see
 * module doc for why nothing server-side does it for this model the way
 * `provisionAppClient` does for a signing key. */
export async function createWebhookEndpoint(
  fields: CreateWebhookEndpointFields,
): Promise<CreatedWebhookEndpoint> {
  const secret = `whsec_${randomBytes(32).toString("hex")}`;
  const body = JSON.stringify({
    appId: fields.appId,
    url: fields.url,
    eventTypes: packEventTypes(fields.eventTypes),
    secret,
    maskRecipient: fields.maskRecipient,
    maxAttempts: fields.maxAttempts,
  });

  const response = await authedRequest(gatewayUrl("/webhook_endpoints"), {
    method: "POST",
    body,
  });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "createWebhookEndpoint");
  }
  return { endpoint: normalizeEndpoint(parsed as WireEndpoint), secret };
}

export interface UpdateWebhookEndpointFields {
  url?: string | undefined;
  eventTypes?: WebhookEventType[] | undefined;
  maskRecipient?: boolean | undefined;
  active?: boolean | undefined;
  maxAttempts?: number | undefined;
}

/** `PATCH /webhook_endpoints/{id}` with `If-Match`. Deliberately cannot
 * touch `secret`/`prevSecret`/`secretRotatedAt` — [`rotateWebhookSecret`]
 * below is the only path this module exposes for that, the audited
 * procedure built for exactly this write rather than a plain field edit. */
export async function updateWebhookEndpoint(
  id: string,
  etag: string,
  fields: UpdateWebhookEndpointFields,
): Promise<WithEtag<WebhookEndpointRecord>> {
  const { eventTypes, ...rest } = fields;
  const body: Record<string, unknown> = { ...rest };
  if (eventTypes !== undefined) body.eventTypes = packEventTypes(eventTypes);

  const result = await updateWithIfMatch<WireEndpoint>(
    `/webhook_endpoints/${encodeURIComponent(id)}`,
    body,
    etag,
    "updateWebhookEndpoint",
  );
  return { ...result, data: normalizeEndpoint(result.data) };
}

/** `DELETE /webhook_endpoints/{id}`. `WebhookEndpoint` carries `@version`
 * (#59), and as of the cratestack 0.7.16 bump `DELETE` on a `@version`
 * model needs `If-Match`. Pass `etag` (the row's `WithEtag.etag` or a
 * plain `String(version)`) when the caller already has it —
 * `webhooks-screen.tsx`'s `deleteTarget` comes straight from
 * `listWebhookEndpoints`, which already carries `version` — so `rest.ts`'s
 * `deleteResource` sends it directly with no extra round trip. Omit it and
 * `deleteResource` falls back to a `GET` first — see its own doc on
 * [`deleteResource`] for that mechanism and its honestly-stated TOCTOU
 * cost. */
export async function deleteWebhookEndpoint(id: string, etag?: string): Promise<void> {
  return deleteResource(
    `/webhook_endpoints/${encodeURIComponent(id)}`,
    "deleteWebhookEndpoint",
    etag,
  );
}

/**
 * `POST /$procs/rotateWebhookSecret` (#41/#59/#193). Moves the current
 * `secret` to `prevSecret`, mints a fresh `secret`, stamps
 * `secretRotatedAt` — the whole overlap-window mechanism lives server-side
 * in `Procedures::rotate_secret`; this function is a thin call, same shape
 * `jobs.ts`'s `requeueJob` already has for a mutation procedure.
 */
export async function rotateWebhookSecret(endpointId: string): Promise<WebhookEndpointRecord> {
  const response = await authedRequest(gatewayUrl("/$procs/rotateWebhookSecret"), {
    method: "POST",
    body: JSON.stringify({ args: { endpointId } }),
  });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "rotateWebhookSecret");
  }
  return normalizeEndpoint(parsed as WireEndpoint);
}

export interface ListWebhookAttemptsInput {
  endpointId?: string | undefined;
  state?: AttemptState | undefined;
  limit?: number | undefined;
  offset?: number | undefined;
}

export interface ListWebhookAttemptsResult {
  items: WebhookAttemptRecord[];
  totalCount: number;
  hasNextPage: boolean;
  /** True when the fetched window (capped at the server's own 1000-row
   * ceiling) may not contain every attempt matching the filter — same
   * honesty contract `listMessages`/`listJobs` already carry. */
  truncated: boolean;
}

const MAX_SERVER_LIMIT = 1000;
const DEFAULT_LIST_LIMIT = 100;

interface AttemptsPage {
  items: WebhookAttemptRecord[];
  totalCount: number;
  pageInfo: {
    limit: number;
    offset: number | null;
    hasNextPage: boolean;
    hasPreviousPage: boolean;
  };
}

/** Attempts carry no `createdAt`/`updatedAt` at all (`WebhookAttempt` has
 * no `@use(Timestamps)` — AGENTS.md §3.4's own accepted-limitation note on
 * `occurredAt`). `lastAttemptAt`/`deliveredAt` are the only real
 * timestamps, and both are null for a `pending` row that has never been
 * tried — so this module sorts *client-side* on whichever is present,
 * newest first, rather than asking the server for a `sort=` on a column
 * that can't order every row. A `pending` row with neither timestamp sorts
 * last, which is the right place for "hasn't happened yet" in a history
 * view. */
function recencyKey(record: WebhookAttemptRecord): string {
  return record.lastAttemptAt ?? record.deliveredAt ?? "";
}

/**
 * `GET /webhook_attempts`, windowed and filtered the same way `listJobs`
 * documents: no server-side `endpointId`/`state` filter attempted (§2.0's
 * own warning that the REST filter grammar is unreliable for anything but
 * a handful of confirmed scalar columns — never verified live for this
 * model, so this module doesn't gamble on it), filtering happens here, in
 * Node, over a bounded 1000-row window.
 */
export async function listWebhookAttempts(
  input: ListWebhookAttemptsInput = {},
): Promise<ListWebhookAttemptsResult> {
  const limit = input.limit ?? DEFAULT_LIST_LIMIT;
  const offset = input.offset ?? 0;

  const response = await authedRequest(
    gatewayUrl("/webhook_attempts", { limit: MAX_SERVER_LIMIT }),
    {
      method: "GET",
    },
  );
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listWebhookAttempts");
  }
  const page = parsed as AttemptsPage;
  const window = page.items;

  const filtered = window
    .filter((record) => {
      if (input.endpointId !== undefined && record.endpointId !== input.endpointId) return false;
      if (input.state !== undefined && record.state !== input.state) return false;
      return true;
    })
    .sort((a, b) => (recencyKey(a) < recencyKey(b) ? 1 : recencyKey(a) > recencyKey(b) ? -1 : 0));

  const pageSlice = filtered.slice(offset, offset + limit);

  return {
    items: pageSlice,
    totalCount: filtered.length,
    hasNextPage: offset + limit < filtered.length,
    truncated: window.length >= MAX_SERVER_LIMIT,
  };
}

/**
 * `POST /$procs/replayWebhookAttempt` (#43/#191/#193). Only `failed`/`dead`
 * attempts are replayable — `replay_attempt`'s own `409 Conflict` on
 * anything else is the real guard (Postgres decides, this UI proposes, the
 * same "requeueJob" precedent `jobs.ts` already documents); this function
 * doesn't pre-check state, it just calls through and lets the server's own
 * answer be authoritative.
 */
export async function replayWebhookAttempt(attemptId: string): Promise<WebhookAttemptRecord> {
  const response = await authedRequest(gatewayUrl("/$procs/replayWebhookAttempt"), {
    method: "POST",
    body: JSON.stringify({ args: { attemptId } }),
  });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "replayWebhookAttempt");
  }
  return parsed as WebhookAttemptRecord;
}
