import "server-only";

// `GET /users`, `PATCH /users/{id}`, `DELETE /users/{id}`, and
// `POST /$procs/provisionUser` — #58's Users screen.
//
// # Provisioning is a procedure, not `POST /users`, for the identical
// # reason `provisionAppClient` isn't a plain create
//
// `User.create`'s own `@@allow` (`hasRole('owner') || hasRole('admin')`)
// would actually admit a direct `POST /users` — but `UserCredential`
// (holding the Argon2id password hash) is `hasRole('system')`-only on
// every action, so a console-created `User` row with no procedure to
// attach a credential to would be a permanently unusable account: no
// password, no way to ever log in. `provisionUser` does both writes in one
// transaction and returns the one-time password exactly once — see
// `crates/sms-api/src/procedures.rs::provision_console_user`'s own doc for
// the mixed-context-write mechanism, and `schema.cstack`'s own comment on
// `provisionUser` for why there is no rotate/reset counterpart (see
// `OPEN_QUESTIONS.md` for that gap, named rather than silently absent).

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";
import { fetchWithEtag, updateWithIfMatch, type WithEtag } from "./rest";

export interface UserRecord {
  id: string;
  subject: string;
  email: string;
  displayName: string;
  roleKey: string;
  active: boolean;
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

type UndiciResponse = Awaited<ReturnType<typeof undiciFetch>>;

function gatewayUrl(path: string, query: Record<string, string | number | undefined>): string {
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

async function authedRequest(
  url: string,
  init: { method: "GET" | "POST" | "DELETE"; body?: string },
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

/** The wire sends an explicit JSON `null` for an absent nullable column,
 * never an omitted key — confirmed live in this same PR (`lastLoginAt`
 * rendered as `1970-01-01` — `new Date(null)`'s own epoch — for an account
 * that had never logged in, because nothing here converted the wire's
 * `null` to this module's own `undefined`-only convention before a
 * `!== undefined` check in `users-screen.tsx` let it through). Same
 * finding `routes.ts`/`route-simulator.ts`/`providers.ts` each already
 * document for their own nullable fields; this module just hadn't needed
 * it demonstrated yet. */
function normalizeUser<T extends Partial<UserRecord>>(record: T): T {
  return { ...record, lastLoginAt: record.lastLoginAt ?? undefined };
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
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listUsers");
  }
  const page = parsed as UsersPage;
  return page.items.map(normalizeUser).map(pickListFields);
}

export async function getUserById(id: string): Promise<WithEtag<UserRecord> | null> {
  const result = await fetchWithEtag<UserRecord>(`/users/${encodeURIComponent(id)}`, "getUser");
  return result === null ? null : { ...result, data: normalizeUser(result.data) };
}

export interface UpdateUserFields {
  displayName?: string | undefined;
  roleKey?: string | undefined;
  active?: boolean | undefined;
}

/** `PATCH /users/{id}` with `If-Match: etag`. `email`/`subject` are
 * deliberately not editable from this form — `email` is this account's own
 * login identity and `subject` is the OP's own `sub` claim source
 * (`sms_auth::login`'s doc: "`User.id` as `external_id`"); changing either
 * casually from an edit dialog risks locking the account's own owner out,
 * which is a bigger decision than this screen should make implicitly. */
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
 * only. `User` carries `@@soft_delete`. */
export async function deleteUser(id: string): Promise<void> {
  const url = gatewayUrl(`/users/${encodeURIComponent(id)}`, {});
  const response = await authedRequest(url, { method: "DELETE" });
  if (!response.ok) {
    const parsed = await parseJsonBody(response);
    throw mapGatewayError(response.status, parsed, "deleteUser");
  }
}

export interface ProvisionUserResult {
  userId: string;
  email: string;
  roleKey: string;
  /** Shown exactly once — see this module's own doc and
   * `crates/sms-api/src/procedures.rs::provision_console_user`'s doc for
   * why there is no way to retrieve it again short of provisioning a
   * replacement account. Never persisted by this module beyond the one
   * response it arrives in. */
  password: string;
}

/** `POST /$procs/provisionUser` (#52/#58). */
export async function provisionUser(
  email: string,
  displayName: string,
  roleKey: string,
): Promise<ProvisionUserResult> {
  const url = new URL("/$procs/provisionUser", env.SMS_API_URL).toString();
  const response = await authedRequest(url, {
    method: "POST",
    body: JSON.stringify({ args: { email, displayName, roleKey } }),
  });
  const parsed = await parseJsonBody(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "provisionUser");
  }
  return parsed as ProvisionUserResult;
}
