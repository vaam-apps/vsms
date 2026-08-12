import "server-only";

// `GET /providers`, `GET /providers/{id}`, `PATCH /providers/{id}` — #54's
// Providers screen. Same temporary hand-written seam as `client.ts`/
// `messages.ts`/`jobs.ts` (see `client.ts`'s module doc for why: T3/
// `packages/sms-client` is blocked on an upstream cratestack release).
//
// # Why this is real today and `messages.ts`'s own app-scoping isn't the
// shape here
//
// `Provider` carries no `appId` — it's a system-wide resource, the same
// shape `Job`/`WorkerLockInfo` already have (`jobs.ts`/`workers.ts`'s own
// module docs). `schema.cstack`'s `Provider.read` `@@allow` gained
// `auth().kind == "app"` in this same PR specifically so the console's own
// machine credential can list/read it — before that, only a human role
// (`owner`/`admin`/`operator`/`auditor`) could, and none of those exist yet
// (#97/#98's scope cut). `router::PROVIDER_ROUTE_READ_ROUTES` gates
// `GET /providers`/`GET /providers/{id}` on a `provider:read` scope at
// Layer 2 — the real perimeter, same shape `job:read`/`worker:read`
// already established.
//
// # `updateProvider` — reachable today, as of #211
//
// `Provider.update`'s own `@@allow` is `hasRole('owner') ||
// hasRole('admin') || hasRole('operator')` — no `auth().kind == "app"` at
// all. #194 (human login) landed and `GatewayAuth` genuinely resolves a
// real `hasRole(...)`-meaningful context for a human token; #211 closed the
// remaining gap this paragraph used to describe — every function in this
// file now goes through `resolveUpstreamAccessToken()`
// (`./request-credential.ts`), which forwards the signed-in human's own
// session token (`admin/lib/oidc.ts::Session.accessToken`) for an ordinary
// admin-console request, rather than this console's own separate
// `SMS_CONSOLE_CLIENT_ID` machine credential (`kind: "app"`, `role: "app"`
// always — still used elsewhere, deliberately, see `token.ts`'s own doc).
// `updateProvider` was built and tested (`rest.test.ts`) against a fake
// upstream before this landed; #211's own PR description carries the live
// proof against a real gateway with a real signed-in `owner`.
//
// `GET /providers` on a non-`@@paged` model returns a bare JSON array, not
// `{ items, totalCount, pageInfo }` — confirmed by reading
// `cratestack-macros-0.7.10/src/axum/model/prep.rs`'s own
// `list_response_type` (`Vec<ProjectedValue>` when `!paged`), not assumed
// from `messages.ts`/`jobs.ts`'s own envelope (both `@@paged` models).
//
// `healthCheckedAt: null` (an explicit JSON `null`, not an omitted key) is
// what a real `GET /providers` sends for a row that's never been probed —
// confirmed live against a real gateway (`just demo`), the same finding
// `routes.ts`'s own module doc records at length for `Route`'s four
// `match*` columns, against `messages-screen.tsx`'s own contradicting claim
// for `Message.stateReason`. [`normalizeProvider`] is the equivalent
// single normalization point here — one field today, but the same
// discipline `routes.ts` needed for five.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";
import { fetchWithEtag, updateWithIfMatch, type WithEtag } from "./rest";

/** `schema.cstack`'s `ProviderKind`, verbatim. */
export type ProviderKind = "orange_cm_http" | "mtn_http" | "aggregator_http" | "smpp";

/** `schema.cstack`'s `ProviderState`, verbatim. */
export type ProviderState = "active" | "degraded" | "disabled" | "draining";

/** The full row shape `GET /providers`/`GET /providers/{id}` can return —
 * transcribed from `schema.cstack`'s `Provider` model. `credentialRef` is a
 * *reference* (e.g. `"env:ORANGE_CM_CLIENT_ID"`), never the credential
 * itself — see the model's own field, not `@sensitive`/`@pii` (neither is
 * set on it), so nothing here withholds it, but nothing here treats it as
 * safe to display carelessly either. */
export interface ProviderRecord {
  id: string;
  key: string;
  displayName: string;
  kind: ProviderKind;
  state: ProviderState;
  config: string;
  credentialRef: string;
  maxTps: number;
  maxDailySubmissions: number;
  supportsDlr: boolean;
  supportsAlphaSender: boolean;
  supportsUcs2: boolean;
  supportsConcat: boolean;
  /** `Decimal` on the wire — kept as a string, never parsed to `number`,
   * per this project's money-safety convention. */
  costPerSegmentXaf: string;
  healthCheckedAt?: string | undefined;
  healthy: boolean;
  version: number;
  createdAt: string;
  updatedAt: string;
}

const LIST_FIELDS = [
  "id",
  "key",
  "displayName",
  "kind",
  "state",
  "maxTps",
  "maxDailySubmissions",
  "costPerSegmentXaf",
  "healthy",
  "healthCheckedAt",
  "version",
  "updatedAt",
] as const;

export type ProviderListItem = Pick<ProviderRecord, (typeof LIST_FIELDS)[number]>;

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function providersUrl(path: string, query: Record<string, string | number | undefined>): string {
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

/** `GET` with a Bearer token, retrying once on an unexpected 401 — same
 * shape as `jobs.ts`'s own `authedRequest`, duplicated for the same
 * "independently-replaceable seam" reason that module's doc gives. */
/** See module doc — the wire's explicit `null` becomes this seam's own
 * `undefined`-only convention. */
function normalizeProvider<T extends Partial<ProviderRecord>>(row: T): T {
  return { ...row, healthCheckedAt: row.healthCheckedAt ?? undefined };
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

/**
 * `GET /providers`, unpaged (see module doc) — every `Provider` row this
 * deployment has, projected to [`LIST_FIELDS`]. No filtering/paging: the
 * number of providers in a real deployment is small (one per adapter/
 * aggregator contract), unlike `Message`/`Job`.
 */
export async function listProviders(): Promise<ProviderListItem[]> {
  const url = providersUrl("/providers", { fields: LIST_FIELDS.join(",") });
  const response = await authedGet(url);
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listProviders");
  }
  return (parsed as ProviderListItem[]).map(normalizeProvider);
}

/**
 * `GET /providers/{id}` with its `ETag` captured — the version a
 * subsequent [`updateProvider`] call must echo back as `If-Match`. `null`
 * on a 404.
 */
export async function getProviderById(id: string): Promise<WithEtag<ProviderRecord> | null> {
  const result = await fetchWithEtag<ProviderRecord>(
    `/providers/${encodeURIComponent(id)}`,
    "getProvider",
  );
  return result === null ? null : { ...result, data: normalizeProvider(result.data) };
}

/** The operationally-relevant fields this screen lets a human edit —
 * deliberately narrower than every column `PATCH /providers/{id}` would
 * accept. `key`/`kind`/`config`/`credentialRef` are infrastructure wiring,
 * set once at provisioning time (`sms-gateway seed-dispatch`) and risky to
 * change from a list-screen edit form; left read-only in the detail view
 * instead. A named scope cut, not an oversight — see `providers-screen.tsx`'s
 * own doc. */
export interface UpdateProviderFields {
  displayName?: string | undefined;
  state?: ProviderState | undefined;
  maxTps?: number | undefined;
  maxDailySubmissions?: number | undefined;
  costPerSegmentXaf?: string | undefined;
}

/**
 * `PATCH /providers/{id}` with `If-Match: etag`. See this module's own doc
 * for why this 403s against a real gateway regardless of who is logged
 * into the browser — built and tested against that reality, not hidden
 * behind a flag.
 */
export async function updateProvider(
  id: string,
  etag: string,
  fields: UpdateProviderFields,
): Promise<WithEtag<ProviderRecord>> {
  return updateWithIfMatch<ProviderRecord>(
    `/providers/${encodeURIComponent(id)}`,
    fields,
    etag,
    "updateProvider",
  );
}
