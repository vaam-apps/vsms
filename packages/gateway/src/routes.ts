import "server-only";

// `GET /routes`, `GET /routes/{id}`, `POST /routes`, `PATCH /routes/{id}`,
// `DELETE /routes/{id}` — #54's Routes screen. Same temporary hand-written
// seam as `providers.ts` (see that module's own doc and `client.ts`'s for
// the full reasoning).
//
// `Route` shares `Provider`'s exact shape for this ticket: no `appId`,
// `read` gained `auth().kind == "app"` in this PR so the console's own
// credential can list it, gated at Layer 2 by `router::
// PROVIDER_ROUTE_READ_ROUTES` on `route:read`. Writes
// (`create`/`update`/`delete`) stay `hasRole('owner') || hasRole('admin')`
// only — untouched by this PR — so every write function below is real,
// tested code that 403s against a real gateway regardless of who is
// logged into the browser, the identical situation `providers.ts`'s
// `updateProvider` documents at length (#194 landed a real human login,
// but nothing forwards that session's own token through this package —
// every call here still authenticates as the one static machine
// credential). `routes-screen.tsx` states this on screen.
//
// `GET /routes` returns a bare JSON array (non-`@@paged`, same as
// `GET /providers` — see `providers.ts`'s own doc for how that was
// confirmed).
//
// **A nullable optional column is serialised as an explicit JSON `null`,
// not omitted — confirmed live, against `messages-screen.tsx`'s own claim
// for `Message` ("a message with no `stateReason` simply has no
// `stateReason` key").** Caught running the real Routes screen against a
// real gateway (`just demo`): `demo catch-all`'s own wildcard route came
// back as `{"matchOperator":null,"matchClass":null,"matchAppId":null,
// "matchPrefix":null,...}`, not with those keys absent. A first draft of
// `routes-screen.tsx`'s own `predicateSummary` checked `!== undefined`,
// which is `true` for a JSON `null` — every wildcard route rendered as
// "operator=null, class=null, ..." instead of "matches anything".
// [`normalizeRoute`] is the one place that turns the wire's `null` into
// this package's own `undefined`-only convention (`field?: T | undefined`,
// matching every other type in this seam), so every consumer downstream —
// `routes-screen.tsx` included — can keep using a plain `!== undefined`
// check rather than every call site needing to remember `!= null`.
// `Provider.healthCheckedAt` shows the identical shape live
// (`"healthCheckedAt":null`) — `providers.ts`'s own type reflects it too,
// even though nothing currently renders that field.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { deleteResource, fetchWithEtag, postJson, updateWithIfMatch, type WithEtag } from "./rest";
import { getAccessToken, invalidateAccessToken } from "./token";

/** `schema.cstack`'s `OperatorCode`, verbatim — duplicated from `client.ts`
 * rather than imported, same reasoning `jobs.ts`'s own `JobState` duplicate
 * gives (`@vsms/gateway` is server-only; this avoids a needless coupling
 * between two otherwise-independent files in the same temporary seam). */
export type RouteOperatorCode = "mtn" | "orange" | "camtel" | "nexttel" | "unknown";

/** `schema.cstack`'s `MessageClass`, verbatim. */
export type RouteMessageClass = "otp" | "transactional" | "notification" | "marketing";

/** The full row shape `GET /routes`/`GET /routes/{id}` can return —
 * transcribed from `schema.cstack`'s `Route` model. */
export interface RouteRecord {
  id: string;
  name: string;
  priority: number;
  weight: number;
  enabled: boolean;
  matchOperator?: RouteOperatorCode | undefined;
  matchClass?: RouteMessageClass | undefined;
  matchAppId?: string | undefined;
  matchPrefix?: string | undefined;
  providerId: string;
  failoverRouteId?: string | undefined;
  version: number;
  createdAt: string;
  updatedAt: string;
}

const LIST_FIELDS = [
  "id",
  "name",
  "priority",
  "weight",
  "enabled",
  "matchOperator",
  "matchClass",
  "matchAppId",
  "matchPrefix",
  "providerId",
  "failoverRouteId",
  "version",
  "updatedAt",
] as const;

export type RouteListItem = Pick<RouteRecord, (typeof LIST_FIELDS)[number]>;

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function routesUrl(path: string, query: Record<string, string | number | undefined>): string {
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

/** Converts the wire's explicit `null` (see module doc) into `undefined`
 * for every nullable column this seam re-exposes — the one normalization
 * point, so nothing downstream needs its own `!= null` check. `T` is
 * always either `RouteRecord` or `RouteListItem` (a subset of the same
 * fields), so a single generic covers both list and detail. */
function normalizeRoute<T extends Partial<RouteRecord>>(row: T): T {
  return {
    ...row,
    matchOperator: row.matchOperator ?? undefined,
    matchClass: row.matchClass ?? undefined,
    matchAppId: row.matchAppId ?? undefined,
    matchPrefix: row.matchPrefix ?? undefined,
    failoverRouteId: row.failoverRouteId ?? undefined,
  };
}

async function authedGet(url: string): Promise<UndiciResponse> {
  const attempt = async (): Promise<UndiciResponse> => {
    const token = await getAccessToken();
    return undiciFetch(url, {
      method: "GET",
      headers: { accept: "application/json", authorization: `Bearer ${token}` },
      dispatcher: gatewayAgent(),
    });
  };

  let response = await attempt();
  if (response.status === 401) {
    invalidateAccessToken();
    response = await attempt();
  }
  return response;
}

/** `GET /routes`, unpaged — see module doc. Sorted by `priority` desc, then
 * `id` asc server-side is what `sms_routing::select_route` itself relies on
 * for a reproducible weighted draw (`crates/sms-routing/src/engine.rs`'s
 * own doc); this list doesn't need that ordering for display (it re-sorts
 * for humans below), but requesting it anyway keeps this function honest
 * about what the engine actually reads if a caller ever wants to eyeball
 * "the exact order the engine sees". */
export async function listRoutes(): Promise<RouteListItem[]> {
  const url = routesUrl("/routes", {
    fields: LIST_FIELDS.join(","),
    sort: "-priority",
  });
  const response = await authedGet(url);
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listRoutes");
  }
  return (parsed as RouteListItem[]).map(normalizeRoute);
}

/** `GET /routes/{id}` with its `ETag` captured. `null` on a 404. */
export async function getRouteById(id: string): Promise<WithEtag<RouteRecord> | null> {
  const result = await fetchWithEtag<RouteRecord>(`/routes/${encodeURIComponent(id)}`, "getRoute");
  return result === null ? null : { ...result, data: normalizeRoute(result.data) };
}

export interface CreateRouteFields {
  name: string;
  priority: number;
  weight: number;
  enabled: boolean;
  matchOperator?: RouteOperatorCode | undefined;
  matchClass?: RouteMessageClass | undefined;
  matchAppId?: string | undefined;
  matchPrefix?: string | undefined;
  providerId: string;
  failoverRouteId?: string | undefined;
}

/** `POST /routes`. See module doc for why this 403s against a real
 * gateway regardless of who is logged into the browser. */
export async function createRoute(fields: CreateRouteFields): Promise<RouteRecord> {
  return postJson<RouteRecord>("/routes", fields, "createRoute");
}

/** Every field optional for a `PATCH` — spelled out explicitly, not
 * `Partial<CreateRouteFields>`: under `tsconfig.base.json`'s
 * `exactOptionalPropertyTypes`, `Partial<T>` makes a key optional without
 * widening its *value* type to include `undefined`, so a zod-parsed
 * `{ name: string | undefined }` (from `packages/api/src/routers/routes.ts`'s
 * own `.optional()` fields) wouldn't satisfy it — the same
 * `field?: T | undefined` pattern `client.ts`'s own module doc documents. */
export interface UpdateRouteFields {
  name?: string | undefined;
  priority?: number | undefined;
  weight?: number | undefined;
  enabled?: boolean | undefined;
  matchOperator?: RouteOperatorCode | undefined;
  matchClass?: RouteMessageClass | undefined;
  matchAppId?: string | undefined;
  matchPrefix?: string | undefined;
  providerId?: string | undefined;
  failoverRouteId?: string | undefined;
}

/** `PATCH /routes/{id}` with `If-Match: etag`. Same reachability caveat as
 * [`createRoute`]. */
export async function updateRoute(
  id: string,
  etag: string,
  fields: UpdateRouteFields,
): Promise<WithEtag<RouteRecord>> {
  return updateWithIfMatch<RouteRecord>(
    `/routes/${encodeURIComponent(id)}`,
    fields,
    etag,
    "updateRoute",
  );
}

/** `DELETE /routes/{id}` — no `If-Match` needed (`rest.ts`'s own
 * `deleteResource` doc). Same reachability caveat as [`createRoute`]. */
export async function deleteRoute(id: string): Promise<void> {
  return deleteResource(`/routes/${encodeURIComponent(id)}`, "deleteRoute");
}
