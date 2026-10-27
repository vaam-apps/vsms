// `GET /users`, `PATCH /users/{id}`, `DELETE /users/{id}`, and
// `POST /$procs/provisionUser` — #58's Users screen.
//
// # Provisioning is a procedure, not `POST /users`
//
// `User.create`'s own `@@allow` (`hasRole('owner') || hasRole('admin')`)
// would admit a direct `POST /users`, but every other path to a `User`
// assumes `subject` is the row's own `id`, which a caller cannot know before
// the row exists. `provisionUser` writes the row and corrects `subject` in
// one transaction — see `backends/crates/sms-api/src/procedures.rs::
// provision_console_user`. It makes **no credential**: since VS-4 there is no
// password, token or recovery code, so nothing is returned once. The person
// signs in by pairing a browser key (a vaam admin phone, or
// `sms-gateway pair-console-key` as the break-glass), which attaches to this
// row (`docs/design/console-pairing.md`).

import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { deleteResource, fetchWithEtag, updateWithIfMatch, type WithEtag } from "./rest";
import { GATEWAY_ORIGIN, gatewayFetch } from "./upstream";

export interface UserRecord {
  id: string;
  subject: string;
  email: string;
  displayName: string;
  roleKey: string;
  active: boolean;
  /** The wire sends an explicit JSON `null` here for an account that has
   * never logged in, never an omitted key — confirmed live in the PR that
   * added this module (`lastLoginAt` rendered as `1970-01-01`,
   * `new Date(null)`'s own epoch, before the fix). #221 replaced this
   * module's own single-field `normalizeUser` with `frontends/packages/gateway/src/
   * json.ts`'s shared seam, applied once to every response this package
   * parses rather than per model. */
  lastLoginAt?: string | undefined;
  mfaEnrolled: boolean;
  version: number;
  createdAt: string;
  updatedAt: string;
}

const LIST_FIELDS = [
  "id",
  "email",
  "displayName",
  "roleKey",
  "active",
  "lastLoginAt",
  "mfaEnrolled",
  "version",
  "createdAt",
] as const;

export type UserListItem = Pick<UserRecord, (typeof LIST_FIELDS)[number]>;

function gatewayUrl(path: string, query: Record<string, string | number | undefined>): string {
  const url = new URL(path, GATEWAY_ORIGIN);
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

async function authedRequest(
  url: string,
  init: { method: "GET" | "POST" | "DELETE"; body?: string },
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

interface UsersPage {
  items: UserRecord[];
  totalCount: number;
  pageInfo: {
    limit: number;
    offset: number | null;
    hasNextPage: boolean;
    hasPreviousPage: boolean;
  };
}

function pickListFields(record: UserRecord): UserListItem {
  const out = {} as UserListItem;
  for (const field of LIST_FIELDS) {
    // biome-ignore lint/suspicious/noExplicitAny: narrowing a mapped-tuple key back to its own field
    (out as any)[field] = record[field];
  }
  return out;
}

/** `GET /users`, one bounded page — a real deployment's staff roster is
 * small (the design doc's own `owner` guidance: "1-2 humans"), so this
 * fetches a single generous window rather than building real pagination,
 * the same scope `apps.ts` accepts. */
export async function listUsers(): Promise<UserListItem[]> {
  const url = gatewayUrl("/users", { limit: 500, sort: "email", fields: LIST_FIELDS.join(",") });
  const response = await authedRequest(url, { method: "GET" });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listUsers");
  }
  const page = parsed as UsersPage;
  return page.items.map(pickListFields);
}

export async function getUserById(id: string): Promise<WithEtag<UserRecord> | null> {
  return fetchWithEtag<UserRecord>(`/users/${encodeURIComponent(id)}`, "getUser");
}

export interface UpdateUserFields {
  displayName?: string | undefined;
  roleKey?: string | undefined;
  active?: boolean | undefined;
}

/** `PATCH /users/{id}` with `If-Match: etag`. `email`/`subject` are
 * deliberately not editable from this form — `email` is how the break-glass
 * `pair-console-key --user` finds this account and `subject` is its own `id`;
 * changing either casually from an edit dialog risks locking the account's
 * owner out, which is a bigger decision than this screen should make
 * implicitly. */
export async function updateUser(
  id: string,
  etag: string,
  fields: UpdateUserFields,
): Promise<WithEtag<UserRecord>> {
  return updateWithIfMatch<UserRecord>(
    `/users/${encodeURIComponent(id)}`,
    fields,
    etag,
    "updateUser",
  );
}

/** `DELETE /users/{id}` — `User.delete`'s own `@@allow` is `hasRole('owner')`
 * only. `User` carries `@@soft_delete`.
 *
 * **Delegates to `rest.ts`'s `deleteResource` as of the cratestack 0.7.16
 * bump, rather than hand-rolling the same request a second time.** `User`
 * also carries `@version` (#59) and, per cratestack 0.7.13
 * (cratestack#519), `DELETE` on a `@version` model now requires
 * `If-Match`. Pass `etag` (the row's `WithEtag.etag`, e.g. from
 * `getUserById`) when the caller already has it — `users-screen.tsx`'s
 * `UserDetailDrawer` always does, from the same `GET` that populated the
 * edit form — so `deleteResource` sends it directly with no extra round
 * trip. Omit it and `deleteResource` falls back to a `GET` first — see
 * `rest.ts`'s `deleteResource` doc for that mechanism and its
 * honestly-stated TOCTOU cost. */
export async function deleteUser(id: string, etag?: string): Promise<void> {
  return deleteResource(`/users/${encodeURIComponent(id)}`, "deleteUser", etag);
}

export interface ProvisionUserResult {
  userId: string;
  email: string;
  roleKey: string;
}

/** `POST /$procs/provisionUser` (#52/#58): makes the `User` row, no credential. */
export async function provisionUser(
  email: string,
  displayName: string,
  roleKey: string,
): Promise<ProvisionUserResult> {
  const url = new URL("/$procs/provisionUser", GATEWAY_ORIGIN).toString();
  const response = await authedRequest(url, {
    method: "POST",
    body: JSON.stringify({ args: { email, displayName, roleKey } }),
  });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "provisionUser");
  }
  return parsed as ProvisionUserResult;
}
