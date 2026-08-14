import "server-only";

// `GET /apps`, `GET /apps/{id}`, `POST /apps`, `PATCH /apps/{id}`,
// `DELETE /apps/{id}` — #52's Apps screen. Same temporary hand-written seam
// as `providers.ts`/`routes.ts` (see `client.ts`'s module doc for why: T3/
// `packages/sms-client` is blocked on an upstream cratestack release).
//
// `App` is `@@paged`, unlike `Provider`/`Route` — the response envelope is
// `{ items, totalCount, pageInfo }`, the same shape `jobs.ts`/`messages.ts`
// already document at length, not the bare array `providers.ts` sees for a
// non-paged model. Every call here goes through `resolveUpstreamAccessToken`
// (`./request-credential.ts`), so an ordinary console request reaches
// `sms-api` as the signed-in human, not this console's own machine
// credential — `App.update`/`App.delete`'s own `@@allow` (`hasRole('owner')`/
// `hasRole('admin')` only, no `auth().kind == "app"` clause) has never been
// reachable by the machine credential at all, so write screens here only
// became meaningful once #211 landed.
//
// `ipAllowlist` is sentinel-packed the same way `AppClient.scopes`/
// `OauthClient.grantTypes` are (`sms_core::pack`'s convention: a leading and
// trailing separator, `" "` for empty) — confirmed by every Rust seed
// fixture in this tree writing `ipAllowlist: " "` for "no restriction",
// never `""`. This module's own [`packIpAllowlist`]/[`unpackIpAllowlist`]
// replicate that convention client-side; sending an unpacked string (no
// surrounding spaces) would still round-trip through `unpack`'s own
// tolerant reader, but would break `.contains(" <cidr> ")`-shaped exact
// membership checks anywhere else in this codebase that assumes the packed
// form — cheap to get right here rather than relying on every future
// reader being equally tolerant.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";
import { fetchWithEtag, postJson, updateWithIfMatch, type WithEtag } from "./rest";

/** The full row shape `GET /apps`/`GET /apps/{id}` can return — transcribed
 * from `schema.cstack`'s `App` model. */
export interface AppRecord {
  id: string;
  name: string;
  slug: string;
  description?: string | undefined;
  defaultSenderIdId?: string | undefined;
  monthlyQuota: number;
  /** Sentinel-packed CIDR/IP list — see module doc. Use
   * [`unpackIpAllowlist`] to read it, [`packIpAllowlist`] to write it back. */
  ipAllowlist: string;
  transliterateToGsm7: boolean;
  active: boolean;
  version: number;
  createdAt: string;
  updatedAt: string;
}

const LIST_FIELDS = [
  "id",
  "name",
  "slug",
  "monthlyQuota",
  "transliterateToGsm7",
  "active",
  "version",
  "updatedAt",
] as const;

export type AppListItem = Pick<AppRecord, (typeof LIST_FIELDS)[number]>;

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function gatewayUrl(path: string, query: Record<string, string | number | undefined>): string {
  const url = new URL(path, env.SMS_API_URL);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
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

interface AppsPage {
  items: AppRecord[];
  totalCount: number;
  pageInfo: {
    limit: number;
    offset: number | null;
    hasNextPage: boolean;
    hasPreviousPage: boolean;
  };
}

function pickListFields(record: AppRecord): AppListItem {
  const out = {} as AppListItem;
  for (const field of LIST_FIELDS) {
    // biome-ignore lint/suspicious/noExplicitAny: narrowing a mapped-tuple key back to its own field
    (out as any)[field] = record[field];
  }
  return out;
}

/** Splits a sentinel-packed multi-value column back into its entries —
 * tolerant of the packed form, the unsentinelled form, and repeated
 * separators, mirroring `sms_core::unpack`'s own documented leniency. */
export function unpackIpAllowlist(packed: string): string[] {
  return packed.split(/\s+/).filter((entry) => entry.length > 0);
}

/** The inverse — packs a list of CIDR/IP entries back into the sentinel
 * form (`sms_core::pack`'s convention: leading and trailing separator, a
 * single space for an empty list). */
export function packIpAllowlist(entries: string[]): string {
  const cleaned = entries.map((entry) => entry.trim()).filter((entry) => entry.length > 0);
  return cleaned.length === 0 ? " " : ` ${cleaned.join(" ")} `;
}

/** `GET /apps`, one bounded page — `App` is `@@paged` but a real deployment
 * has few of them (one per integrated product), so this fetches a single
 * generous window rather than building real cursor pagination, the same
 * scope `providers.ts` accepts for an unpaged model. */
export async function listApps(): Promise<AppListItem[]> {
  const url = gatewayUrl("/apps", { limit: 500, sort: "name", fields: LIST_FIELDS.join(",") });
  const response = await authedGet(url);
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listApps");
  }
  const page = parsed as AppsPage;
  return page.items.map(pickListFields);
}

/** `GET /apps/{id}` with its `ETag` captured — the version a subsequent
 * [`updateApp`] call must echo back as `If-Match`. `null` on a 404. */
export async function getAppById(id: string): Promise<WithEtag<AppRecord> | null> {
  return fetchWithEtag<AppRecord>(`/apps/${encodeURIComponent(id)}`, "getApp");
}

export interface CreateAppFields {
  name: string;
  slug: string;
  description?: string | undefined;
  monthlyQuota: number;
  ipAllowlist: string;
  transliterateToGsm7: boolean;
}

/** `POST /apps` — `App.create`'s own `@@allow` is `hasRole('owner') ||
 * hasRole('admin')`. `deletedAt`/`defaultSenderIdId` are left out: the
 * former makes no sense on a row that doesn't exist yet, the latter needs a
 * `SenderId` picker this screen doesn't build (#52's own named scope cut —
 * a default sender can be set later once #52's own edit form supports it,
 * or from `sendMessage`'s own explicit `senderId` argument in the
 * meantime). */
export async function createApp(fields: CreateAppFields): Promise<AppRecord> {
  return postJson<AppRecord>(
    "/apps",
    {
      name: fields.name,
      slug: fields.slug,
      description: fields.description,
      monthlyQuota: fields.monthlyQuota,
      ipAllowlist: fields.ipAllowlist,
      transliterateToGsm7: fields.transliterateToGsm7,
    },
    "createApp",
  );
}

export interface UpdateAppFields {
  name?: string | undefined;
  description?: string | undefined;
  monthlyQuota?: number | undefined;
  ipAllowlist?: string | undefined;
  transliterateToGsm7?: boolean | undefined;
  active?: boolean | undefined;
}

/** `PATCH /apps/{id}` with `If-Match: etag`. `slug` is deliberately not
 * editable here — it's the stable identifier other systems (webhook
 * payloads, this console's own URLs) key on, and changing it under a live
 * integration is a bigger decision than this form should make casually. */
export async function updateApp(
  id: string,
  etag: string,
  fields: UpdateAppFields,
): Promise<WithEtag<AppRecord>> {
  return updateWithIfMatch<AppRecord>(`/apps/${encodeURIComponent(id)}`, fields, etag, "updateApp");
}

/** `DELETE /apps/{id}` — `App.delete`'s own `@@allow` is `hasRole('owner')`
 * only, the narrowest write action this screen exposes. `App` carries
 * `@@soft_delete`, so this marks `deletedAt` rather than physically
 * removing the row — existing `Message`/`AppClient` rows referencing it are
 * untouched.
 *
 * **Stale as of the cratestack 0.7.16 bump: `App` also carries `@version`
 * (#59), and cratestack 0.7.13 (cratestack#519) made `DELETE` on a
 * `@version` model require `If-Match` — independent of `@@soft_delete`,
 * per `cratestack-sqlx`'s own `delete_exec.rs` doc comment ("if_match gates
 * on version_column alone ... whether or not it is also soft_delete_column").
 * This function sends none, so it now 412s against a real gateway. Same
 * "not fixed here, tracked separately" reasoning as `rest.ts`'s
 * `deleteResource` doc.** */
export async function deleteApp(id: string): Promise<void> {
  const url = gatewayUrl(`/apps/${encodeURIComponent(id)}`, {});
  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: "DELETE",
      headers: { accept: "application/json", authorization: `Bearer ${token}` },
      dispatcher: gatewayAgent(),
    });
  };
  let response = await attempt();
  if (response.status === 401) {
    invalidateUpstreamAccessToken();
    response = await attempt();
  }
  if (!response.ok) {
    const parsed = await parseGatewayJson(response);
    throw mapGatewayError(response.status, parsed, "deleteApp");
  }
}
