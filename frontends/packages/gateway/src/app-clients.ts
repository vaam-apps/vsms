// `GET /app_clients`, `PATCH /app_clients/{id}`, and
// `POST /$procs/provisionAppClient` — #52's own client-management half of
// the Apps screen.
//
// # Three separate write shapes, not one CRUD form
//
// - **Provision** (`provisionAppClient`, #23, reshaped by VS-4) is the only
//   way an `AppClient` row is ever created over HTTP — `AppClient.create`'s own
//   `@@allow` is `hasRole('system')` only, so no `POST /app_clients` exists for
//   this console to call even in principle. [`provisionClient`] below hits the
//   procedure instead. The caller generates the Ed25519 keypair and sends only
//   the public half; vsms never holds a private key, so nothing secret comes
//   back and there is nothing to show once.
// - **Retire** is a plain `PATCH /app_clients/{id}` (`AppClient.update`'s
//   `@@allow`: `hasRole('owner') || hasRole('admin')`), setting
//   `active: false` and `retiredAt: <now>`. Retiring a *client* is immediate
//   and total: its keys stop working the instant this call succeeds, with no
//   grace period. (Rotating one *key* with an overlap is `registerAppClientKey`
//   then `revokeAppClientKey`, which this screen does not expose.) A caller
//   that wants zero-downtime client replacement has to provision the *new*
//   client first, update its own integration to use it, and only then retire
//   the old one — this screen does not, and structurally cannot, sequence
//   that for them.
// - **List/get** are ordinary `GET`s, no different in shape from
//   `providers.ts`/`apps.ts`.

import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { fetchWithEtag, updateWithIfMatch, type WithEtag } from "./rest";
import { GATEWAY_ORIGIN, gatewayFetch } from "./upstream";

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

function gatewayUrl(path: string, query: Record<string, string | number | undefined>): string {
  const url = new URL(path, GATEWAY_ORIGIN);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

async function authedRequest(
  url: string,
  init: { method: "GET" | "POST"; body?: string },
): Promise<Response> {
  const response = await gatewayFetch(url, {
    method: init.method,
    headers: {
      accept: "application/json",
      ...(init.body !== undefined ? { "content-type": "application/json" } : {}),
    },
    ...(init.body !== undefined ? { body: init.body } : {}),
  });
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
  /** What a signed request from this client resolves to. */
  clientId: string;
  /** The registered `ClientKey` row — the id `revokeAppClientKey` takes. */
  keyId: string;
  /** RFC 9679 thumbprint of the registered public key, lowercase hex: the
   * identity of the key, computed server-side and never taken from the caller. */
  thumbprint: string;
}

/** `POST /$procs/provisionAppClient` (#23, reshaped by VS-4) — makes an
 * `AppClient` and registers the caller's Ed25519 public key (`publicKey`, the
 * raw 32 bytes as unpadded base64url) for it, in one transaction. Nothing
 * secret is generated or returned. See `backends/crates/sms-api/src/
 * procedures.rs`'s own `provision_client` doc. */
export async function provisionClient(
  appId: string,
  label: string,
  scopes: string[],
  publicKey: string,
): Promise<ProvisionClientResult> {
  const url = new URL("/$procs/provisionAppClient", GATEWAY_ORIGIN).toString();
  const response = await authedRequest(url, {
    method: "POST",
    body: JSON.stringify({ args: { appId, label, scopes, publicKey } }),
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
