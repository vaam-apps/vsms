import "server-only";

// `GET /app_clients`, `PATCH /app_clients/{id}`, and
// `POST /$procs/provisionAppClient` — #52's own client-management half of
// the Apps screen.
//
// # Three separate write shapes, not one CRUD form
//
// - **Provision** (`provisionAppClient`, #23) is the only way an
//   `AppClient` row is ever created — `AppClient.create`'s own `@@allow` is
//   `hasRole('system')` only, so no `POST /app_clients` exists for this
//   console to call even in principle. [`provisionClient`] below hits the
//   procedure instead and returns `privateKeyPem` **exactly once** — see
//   its own doc for what "show it once" actually requires from a caller.
// - **Retire** is a plain `PATCH /app_clients/{id}` (`AppClient.update`'s
//   `@@allow`: `hasRole('owner') || hasRole('admin')`), setting
//   `active: false` and `retiredAt: <now>`. This is the **coarse fallback**
//   AGENTS.md's own `#23` section names explicitly, not a true overlap
//   window: there is no per-client key-history model, so retiring a client
//   is immediate and total — the old key stops working the instant this
//   call succeeds, there is no grace period during which both an old and a
//   new key are simultaneously valid. A caller that wants zero-downtime
//   rotation has to provision the *new* client first, update its own
//   integration to use it, and only then retire the old one — this screen
//   does not, and structurally cannot, sequence that for them.
// - **List/get** are ordinary `GET`s, no different in shape from
//   `providers.ts`/`apps.ts`.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";
import { fetchWithEtag, updateWithIfMatch, type WithEtag } from "./rest";

/** The full row shape `GET /app_clients`/`GET /app_clients/{id}` can
 * return — transcribed from `schema.cstack`'s `AppClient` model. `scopes`
 * stays sentinel-packed on this type; use [`unpackScopes`] to read it. */
export interface AppClientRecord {
  id: string;
  appId: string;
  clientId: string;
  label: string;
  scopes: string;
  active: boolean;
  lastUsedAt?: string | undefined;
  retiredAt?: string | undefined;
  version: number;
  createdAt: string;
  updatedAt: string;
}

export function unpackScopes(packed: string): string[] {
  return packed.split(/\s+/).filter((entry) => entry.length > 0);
}

const LIST_FIELDS = [
  "id",
  "appId",
  "clientId",
  "label",
  "scopes",
  "active",
  "lastUsedAt",
  "retiredAt",
  "version",
  "createdAt",
] as const;

export type AppClientListItem = Pick<AppClientRecord, (typeof LIST_FIELDS)[number]>;

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function gatewayUrl(path: string, query: Record<string, string | number | undefined>): string {
  const url = new URL(path, env.SMS_API_URL);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

async function authedRequest(
  url: string,
  init: { method: "GET" | "POST"; body?: string },
): Promise<UndiciResponse> {
  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: init.method,
      headers: {
        accept: "application/json",
        ...(init.body !== undefined ? { "content-type": "application/json" } : {}),
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

interface AppClientsPage {
  items: AppClientRecord[];
  totalCount: number;
  pageInfo: {
    limit: number;
    offset: number | null;
    hasNextPage: boolean;
    hasPreviousPage: boolean;
  };
}

function pickListFields(record: AppClientRecord): AppClientListItem {
  const out = {} as AppClientListItem;
  for (const field of LIST_FIELDS) {
    // biome-ignore lint/suspicious/noExplicitAny: narrowing a mapped-tuple key back to its own field
    (out as any)[field] = record[field];
  }
  return out;
}

/** `GET /app_clients?appId=<id>` — `appId` is a plain non-null scalar
 * column, confirmed filterable by exact-match equality the same way
 * `messages.ts`'s own module doc confirms for `Message.appId` (identical
 * REST grammar, same generated router). `AppClient.read`'s own `@@allow`
 * (`hasRole('owner') || hasRole('admin') || hasRole('developer') ||
 * hasRole('system')`) is narrower than `App.read`'s — an `operator`/
 * `auditor` signed in can see the App list but not its clients, which this
 * screen states rather than silently showing an empty table.
 */
export async function listAppClientsForApp(appId: string): Promise<AppClientListItem[]> {
  const url = gatewayUrl("/app_clients", {
    appId,
    limit: 200,
    sort: "-createdAt",
    fields: LIST_FIELDS.join(","),
  });
  const response = await authedRequest(url, { method: "GET" });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listAppClientsForApp");
  }
  const page = parsed as AppClientsPage;
  return page.items.map(pickListFields);
}

export async function getAppClientById(id: string): Promise<WithEtag<AppClientRecord> | null> {
  return fetchWithEtag<AppClientRecord>(`/app_clients/${encodeURIComponent(id)}`, "getAppClient");
}

export interface ProvisionClientResult {
  clientId: string;
  /** The private key PEM — present in this one response and nowhere else.
   * Never persisted by this module, never logged, never round-tripped
   * through a query cache: see `apps-screen.tsx`'s own doc for what the
   * *caller* must additionally avoid (React state that survives navigation,
   * a toast queue, anything longer-lived than the one dialog showing it). */
  privateKeyPem: string;
}

/** `POST /$procs/provisionAppClient` (#23) — generates a fresh RSA keypair
 * server-side, persists only the derived public JWK, and returns the
 * private key exactly once. See `crates/sms-api/src/procedures.rs`'s own
 * `provision_client` doc for the full mechanism. */
export async function provisionClient(
  appId: string,
  label: string,
  scopes: string[],
): Promise<ProvisionClientResult> {
  const url = new URL("/$procs/provisionAppClient", env.SMS_API_URL).toString();
  const response = await authedRequest(url, {
    method: "POST",
    body: JSON.stringify({ args: { appId, label, scopes } }),
  });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "provisionAppClient");
  }
  return parsed as ProvisionClientResult;
}

/** The coarse retire action — see module doc for why this is not a true
 * overlap window. Sets `active: false` and stamps `retiredAt` to now. */
export async function retireAppClient(
  id: string,
  etag: string,
): Promise<WithEtag<AppClientRecord>> {
  return updateWithIfMatch<AppClientRecord>(
    `/app_clients/${encodeURIComponent(id)}`,
    { active: false, retiredAt: new Date().toISOString() },
    etag,
    "retireAppClient",
  );
}

/** Relabelling a client, or reactivating one previously retired by
 * mistake — the same `PATCH`, different fields. */
export async function updateAppClient(
  id: string,
  etag: string,
  fields: { label?: string | undefined; active?: boolean | undefined },
): Promise<WithEtag<AppClientRecord>> {
  return updateWithIfMatch<AppClientRecord>(
    `/app_clients/${encodeURIComponent(id)}`,
    fields,
    etag,
    "updateAppClient",
  );
}
