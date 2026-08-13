import "server-only";

// `GET /sender_ids`, `GET /sender_ids/{id}`, `POST /sender_ids`,
// `PATCH /sender_ids/{id}`, and the same five verbs for
// `sender_id_registrations` — #53's Sender IDs screen. Same temporary
// hand-written seam as `providers.ts`/`routes.ts` (see `client.ts`'s module
// doc for why: T3/`packages/sms-client` is blocked on an upstream cratestack
// release).
//
// # Two models, one screen, and why they're two REST resources not one
//
// `schema.cstack`'s own comment on `docs/architecture.md` line 426: "sender
// ID status is per-(sender, provider)." `SenderId` is the brand identifier
// itself (global, not app-scoped — any app can request any active sender);
// `SenderIdRegistration` is one row per `(senderId, provider)` pair, each
// with its own `status`/`submittedAt`/`approvedAt`/`reference`/
// `rejectionReason`. Neither model carries the other inline — `GET
// /sender_ids` never embeds its registrations — so this screen fetches both
// lists and joins them client-side (`registrationsBySenderId`), the same
// shape `jobs.ts`'s own module doc describes for filtering it could not push
// server-side.
//
// # Both are unpaged models — no `@@paged` on either in schema.cstack
//
// Same shape `providers.ts`/`routes.ts` document at length: `GET
// /sender_ids`/`GET /sender_id_registrations` return a bare JSON array, no
// `{ items, totalCount, pageInfo }` envelope. No windowing needed — the
// number of sender IDs and their per-provider registrations in any real
// deployment is small (this is brand/compliance data, not traffic).
//
// # `status` is a plain `String`, not a schema enum
//
// `SenderIdRegistration.status` carries no `enum` — only the literal
// `"approved"` is load-bearing anywhere in this codebase
// (`procedures.rs::resolve_sender_id`'s own `APPROVED` const). This module
// does not invent a closed set either; `sender-ids-screen.tsx` renders
// whatever string is stored, with a small fixed palette for the
// conventional `pending`/`submitted`/`approved`/`rejected` values and a
// neutral fallback for anything else — the same "free-form, not
// type-system-enforced" treatment `jobs.ts`'s own `kind` field gets.
//
// # Writes are real, and reachable, as of #211
//
// `SenderId.create`/`.update` and `SenderIdRegistration.create`/`.update`
// (`schema.cstack`) are each `hasRole('owner') || hasRole('admin') ||
// hasRole('operator')` — no `auth().kind == "app"` clause, so (like
// `providers.ts`'s own `updateProvider`) these were never reachable by this
// console's machine credential and need a real human principal. Every
// function below resolves its Bearer token via `resolveUpstreamAccessToken()`
// (`./request-credential.ts`), the signed-in operator's own session token —
// gated further at Layer 2 by `router::SENDER_AND_WEBHOOK_WRITE_ROUTES`
// (`sender:manage`, granted to `owner`/`admin`/`operator` per §5.2's
// seeded roles), matching the shape `providers.ts`'s own doc records for
// `provider:update`. Proven live, not just reasoned about: a real `PATCH
// /sender_id_registrations/{id}` against a `just demo` gateway, signed in as
// the demo `owner`, moved a registration from `approved` to `rejected` with
// a real `rejectionReason`, `version` bumped 0→1 — the exact write path
// #211 unblocked, the same shape that PR's own description proves for
// `Provider`.
//
// # A real framework gap, found live while proving the above, not assumed
//
// AGENTS.md's "Verified toolchain API" section states "nullable setters are
// `Option<Option<T>>`" — true for the Rust delegate builder (`.set(UpdateX
// Input { field: Some(None), .. })`), and **not true for the generated REST
// `PATCH` route this module calls.** Read directly from the vendored source
// before trusting either way: `cratestack-macros-0.7.10/src/model/
// struct_only.rs::struct_field_definition` emits the update-input struct's
// nullable fields as a plain `Option<Option<T>>` with **no** `#[serde(
// deserialize_with = ...)]` — confirmed by grepping the whole crate for
// `deserialize_with`/`double_option` and finding neither anywhere near
// input generation. That is precisely the well-known serde "double Option"
// ambiguity: for a struct field of type `Option<Option<T>>`, serde-derive's
// built-in "missing Option field defaults to None" rule fires on the
// *outer* Option for BOTH an absent key and a present `null` — a bare
// `Option<T>: Deserialize` sees JSON `null` and calls `visit_none()`,
// yielding the outer `None` directly, never reaching the inner `Option`
// to produce `Some(None)`. So **explicit JSON `null` and an omitted key
// are indistinguishable over this REST route — neither can clear a
// nullable column.** Verified live, not just read off the source: `PATCH
// /sender_id_registrations/{id}` with `{"rejectionReason": null, "status":
// "pending", ...}` against a real gateway left `rejectionReason` completely
// unchanged while `status`/`submittedAt` (non-null fields) updated
// normally in the exact same request — the same request proves the route
// itself works and that only the null-clear semantics don't.
//
// No existing caller in this codebase (`providers.ts`/`routes.ts`) had ever
// attempted to clear a nullable field over REST before this was written —
// every prior `UpdateXFields` type only ever *sets* a nullable field to a
// concrete value, never clears one — so nothing had surfaced this before.
// The workaround here, applied consistently to every nullable text field
// this module's write paths touch (`SenderId.notes`, `SenderIdRegistration.
// reference`/`rejectionReason`): represent "clear" as an explicit empty
// string, not `null` — a present, non-null JSON value deserializes
// unambiguously to `Some(Some(""))`, which the route genuinely writes.
// This trades true SQL `NULL` for an empty string in that column; harmless
// here since nothing in `crates/sms-api` queries or branches on
// `NULL`-ness for any of these three columns, only display code does, and
// [`normalizeSenderId`]/[`normalizeRegistration`] below fold `""` back to
// `undefined` on the way out — so a round-trip read never shows the
// difference. Filed as a real, load-bearing finding rather than a silent
// workaround: the next model that needs to clear a *queried-on* nullable
// column over REST cannot use this trick safely and needs the actual
// upstream fix (a `deserialize_with` on the generated update input).

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";
import { fetchWithEtag, postJson, updateWithIfMatch, type WithEtag } from "./rest";

export interface SenderIdRecord {
  id: string;
  value: string;
  kind: string;
  notes?: string | undefined;
  active: boolean;
  version: number;
  createdAt: string;
  updatedAt: string;
}

export interface SenderIdRegistrationRecord {
  id: string;
  senderIdId: string;
  providerId: string;
  status: string;
  submittedAt?: string | undefined;
  approvedAt?: string | undefined;
  reference?: string | undefined;
  rejectionReason?: string | undefined;
  version: number;
  createdAt: string;
  updatedAt: string;
}

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function gatewayUrl(path: string, query: Record<string, string | number | undefined> = {}): string {
  const url = new URL(path, env.SMS_API_URL);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

async function parseJsonBody(response: UndiciResponse): Promise<unknown> {
  const text = await response.text();
  if (text.length === 0) return undefined;
  try {
    return JSON.parse(text);
  } catch {
    return { code: "UNPARSEABLE_RESPONSE", message: text };
  }
}

/** See module doc — the wire's explicit `null` becomes this seam's own
 * `undefined`-only convention (`providers.ts`'s own `normalizeProvider`,
 * `routes.ts`'s `normalizeRoute`), applied to both resources here. Also
 * folds this module's own `""`-means-cleared sentinel (module doc's "a real
 * framework gap" section) back to `undefined`, so a round-trip read can't
 * tell a genuinely-never-set column apart from one this module cleared. */
function normalizeSenderId<T extends Partial<SenderIdRecord>>(row: T): T {
  return { ...row, notes: row.notes === "" ? undefined : (row.notes ?? undefined) };
}

function normalizeRegistration<T extends Partial<SenderIdRegistrationRecord>>(row: T): T {
  return {
    ...row,
    submittedAt: row.submittedAt ?? undefined,
    approvedAt: row.approvedAt ?? undefined,
    reference: row.reference === "" ? undefined : (row.reference ?? undefined),
    rejectionReason: row.rejectionReason === "" ? undefined : (row.rejectionReason ?? undefined),
  };
}

async function authedGet(url: string): Promise<UndiciResponse> {
  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: "GET",
      headers: { accept: "application/json", authorization: `Bearer ${token}` },
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

/** `GET /sender_ids` — every `SenderId` row this deployment has. */
export async function listSenderIds(): Promise<SenderIdRecord[]> {
  const response = await authedGet(gatewayUrl("/sender_ids"));
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listSenderIds");
  }
  return (parsed as SenderIdRecord[]).map(normalizeSenderId);
}

/** `GET /sender_id_registrations` — every registration row this deployment
 * has, across every `SenderId` and `Provider`. `sender-ids-screen.tsx`
 * groups these by `senderIdId` itself (module doc) rather than this module
 * fetching per-sender-id, which would be N+1 requests for what is, in
 * practice, a handful of rows total. */
export async function listSenderIdRegistrations(): Promise<SenderIdRegistrationRecord[]> {
  const response = await authedGet(gatewayUrl("/sender_id_registrations"));
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listSenderIdRegistrations");
  }
  return (parsed as SenderIdRegistrationRecord[]).map(normalizeRegistration);
}

export async function getSenderIdById(id: string): Promise<WithEtag<SenderIdRecord> | null> {
  const result = await fetchWithEtag<SenderIdRecord>(
    `/sender_ids/${encodeURIComponent(id)}`,
    "getSenderId",
  );
  return result === null ? null : { ...result, data: normalizeSenderId(result.data) };
}

export interface CreateSenderIdFields {
  value: string;
  kind: string;
  notes?: string | undefined;
}

/** `POST /sender_ids`. A fresh sender ID starts inactive
 * (`SenderId.active @default(false)`, excluded from the create input per
 * §2.0's own "`@default` excludes the field from `CreateXInput`" rule) —
 * activating it, like registering it with a provider, is a deliberate
 * second step from `sender-ids-screen.tsx`'s edit dialog, not implicit in
 * creation. */
export async function createSenderId(fields: CreateSenderIdFields): Promise<SenderIdRecord> {
  const created = await postJson<SenderIdRecord>(
    "/sender_ids",
    fields,
    "createSenderId",
    undiciFetch,
  );
  return normalizeSenderId(created);
}

export interface UpdateSenderIdFields {
  value?: string | undefined;
  kind?: string | undefined;
  notes?: string | undefined;
  active?: boolean | undefined;
}

export async function updateSenderId(
  id: string,
  etag: string,
  fields: UpdateSenderIdFields,
): Promise<WithEtag<SenderIdRecord>> {
  const result = await updateWithIfMatch<SenderIdRecord>(
    `/sender_ids/${encodeURIComponent(id)}`,
    fields,
    etag,
    "updateSenderId",
  );
  return { ...result, data: normalizeSenderId(result.data) };
}

export interface CreateSenderIdRegistrationFields {
  senderIdId: string;
  providerId: string;
  status: string;
  reference?: string | undefined;
}

/** `POST /sender_id_registrations` — register a sender ID with a provider
 * it doesn't yet have a row for ("Register with a new provider" in the
 * detail panel). `submittedAt` is stamped here, at creation time, rather
 * than left for the operator to set by hand — the act of creating this row
 * *is* the submission. */
export async function createSenderIdRegistration(
  fields: CreateSenderIdRegistrationFields,
): Promise<SenderIdRegistrationRecord> {
  const created = await postJson<SenderIdRegistrationRecord>(
    "/sender_id_registrations",
    { ...fields, submittedAt: new Date().toISOString() },
    "createSenderIdRegistration",
    undiciFetch,
  );
  return normalizeRegistration(created);
}

/**
 * No `| null` on the nullable fields — see module doc's "a real framework
 * gap" section for why: `null` is a silent no-op against this REST route,
 * indistinguishable from omitting the key entirely. `undefined` omits the
 * key (leave untouched); an explicit `""` is this module's own working
 * "clear" sentinel, folded back to `undefined` on read by
 * [`normalizeRegistration`]. `approvedAt` stays date-shaped (never cleared
 * by any caller in this screen) rather than joining the sentinel — nothing
 * here writes an empty string into a `DateTime` column.
 */
export interface UpdateSenderIdRegistrationFields {
  status?: string | undefined;
  submittedAt?: string | undefined;
  approvedAt?: string | undefined;
  reference?: string | undefined;
  rejectionReason?: string | undefined;
}

/** `PATCH /sender_id_registrations/{id}` — the edit dialog's own save, and
 * also what "Resubmit" (`sender-ids-screen.tsx`) uses: `status: "pending"`,
 * a fresh `submittedAt`, `rejectionReason: ""` (cleared — module doc's own
 * sentinel, `null` being a verified no-op here) after an operator has fixed
 * whatever the provider rejected it for. */
export async function updateSenderIdRegistration(
  id: string,
  etag: string,
  fields: UpdateSenderIdRegistrationFields,
): Promise<WithEtag<SenderIdRegistrationRecord>> {
  const result = await updateWithIfMatch<SenderIdRegistrationRecord>(
    `/sender_id_registrations/${encodeURIComponent(id)}`,
    fields,
    etag,
    "updateSenderIdRegistration",
  );
  return { ...result, data: normalizeRegistration(result.data) };
}
