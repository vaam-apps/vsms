import "server-only";

// `GET /roles`, `GET /roles/{id}`, `POST /roles`, `PATCH /roles/{id}`,
// `DELETE /roles/{id}` — #58's Roles screen, `owner`-only for every write
// (`Role.create`/`update`/`delete`'s own `@@allow`).
//
// `Role` is not `@@paged` (unlike `App`/`User`/`AppClient`) — `GET /roles`
// returns a bare array, the same shape `providers.ts`'s own module doc
// documents for `Provider`, confirmed by the identical `list_response_type`
// reasoning (`cratestack-macros-0.7.10/src/axum/model/prep.rs`: `Vec<...>`
// when `!paged`).
//
// # `RESERVED_ROLE_KEYS` — a role keyed `system` or `app` must never exist
//
// `crates/sms-api/src/auth.rs`'s own `RESERVED_ROLE_KEYS` (`["system",
// "app"]`) and a database `CHECK` constraint (`roles_key_not_reserved_check`,
// §2.10) both refuse a `Role` row keyed either literal — `hasRole('system')`
// matching a real human's role would hand them `OauthSigningKey.
// privateKeyPem` and every `UserCredential.passwordHash` through generated
// CRUD. This module's own [`isReservedRoleKey`] is a **third**, client-side
// guard: not load-bearing (the two server-side guards are what actually
// protect this system regardless of what this file does), but it turns a
// mistaken attempt into an immediate, friendly refusal in the create form
// rather than a raw `23514 check_violation` surfacing as an unhelpful
// generic error toast — see `roles-screen.tsx`'s own doc for how it's used.

import { env } from "@vsms/env";
import { fetch as undiciFetch } from "undici";
import { gatewayAgent } from "./dispatcher";
import { mapGatewayError } from "./errors";
import { parseGatewayJson } from "./json";
import { invalidateUpstreamAccessToken, resolveUpstreamAccessToken } from "./request-credential";
import { fetchWithEtag, postJson, updateWithIfMatch, type WithEtag } from "./rest";

export interface RoleRecord {
  id: string;
  key: string;
  label: string;
  description?: string | undefined;
  builtin: boolean;
  /** Sentinel-packed — see [`unpackPermissions`]/[`packPermissions`]. */
  permissions: string;
  version: number;
  createdAt: string;
  updatedAt: string;
}

export function unpackPermissions(packed: string): string[] {
  return packed.split(/\s+/).filter((entry) => entry.length > 0);
}

export function packPermissions(entries: string[]): string {
  const cleaned = entries.map((entry) => entry.trim()).filter((entry) => entry.length > 0);
  return cleaned.length === 0 ? " " : ` ${cleaned.join(" ")} `;
}

/** `Role.key`'s own `@regex` (`^[a-z][a-z0-9_]{2,31}$`) plus the two
 * literals this deployment's own login/RBAC mechanism reserves — see
 * module doc. */
const RESERVED_ROLE_KEYS = new Set(["system", "app"]);
const ROLE_KEY_PATTERN = /^[a-z][a-z0-9_]{2,31}$/;

export function isReservedRoleKey(key: string): boolean {
  return RESERVED_ROLE_KEYS.has(key);
}

export function isValidRoleKeyShape(key: string): boolean {
  return ROLE_KEY_PATTERN.test(key);
}

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
  init: { method: "GET" | "DELETE" },
): Promise<UndiciResponse> {
  const attempt = async (): Promise<UndiciResponse> => {
    const token = await resolveUpstreamAccessToken();
    return undiciFetch(url, {
      method: init.method,
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

/** `GET /roles` — unpaged, every role this deployment has (the built-in
 * six plus whatever `owner` has added). */
export async function listRoles(): Promise<RoleRecord[]> {
  const url = gatewayUrl("/roles", {});
  const response = await authedRequest(url, { method: "GET" });
  const parsed = await parseGatewayJson(response);
  if (!response.ok) {
    throw mapGatewayError(response.status, parsed, "listRoles");
  }
  return parsed as RoleRecord[];
}

export async function getRoleById(id: string): Promise<WithEtag<RoleRecord> | null> {
  return fetchWithEtag<RoleRecord>(`/roles/${encodeURIComponent(id)}`, "getRole");
}

export interface CreateRoleFields {
  key: string;
  label: string;
  description?: string | undefined;
  permissions: string[];
}

/** `POST /roles`. Throws before ever reaching the network if `key` is
 * reserved — the friendly half of the guard described in this module's own
 * doc; the database `CHECK` remains the real backstop regardless. */
export async function createRole(fields: CreateRoleFields): Promise<RoleRecord> {
  if (isReservedRoleKey(fields.key)) {
    throw new Error(`"${fields.key}" is a reserved role key and can never be assigned to a Role`);
  }
  return postJson<RoleRecord>(
    "/roles",
    {
      key: fields.key,
      label: fields.label,
      description: fields.description,
      permissions: packPermissions(fields.permissions),
    },
    "createRole",
  );
}

export interface UpdateRoleFields {
  label?: string | undefined;
  description?: string | undefined;
  permissions?: string[] | undefined;
}

export async function updateRole(
  id: string,
  etag: string,
  fields: UpdateRoleFields,
): Promise<WithEtag<RoleRecord>> {
  return updateWithIfMatch<RoleRecord>(
    `/roles/${encodeURIComponent(id)}`,
    {
      label: fields.label,
      description: fields.description,
      permissions:
        fields.permissions === undefined ? undefined : packPermissions(fields.permissions),
    },
    etag,
    "updateRole",
  );
}

/** `DELETE /roles/{id}` — `owner`-only. A `Role` still referenced by a
 * `User.roleKey` will fail this with a foreign-key violation, surfaced as
 * an ordinary gateway error rather than this module trying to pre-check
 * it — the database is the correct place to enforce that.
 *
 * **Stale as of the cratestack 0.7.16 bump: `Role` also carries `@version`
 * (#59) and, per cratestack 0.7.13 (cratestack#519), `DELETE` on a
 * `@version` model now requires `If-Match` — see `apps.ts`'s
 * `deleteApp`/`rest.ts`'s `deleteResource` doc for the mechanism.** */
export async function deleteRole(id: string): Promise<void> {
  const url = gatewayUrl(`/roles/${encodeURIComponent(id)}`, {});
  const response = await authedRequest(url, { method: "DELETE" });
  if (!response.ok) {
    const parsed = await parseGatewayJson(response);
    throw mapGatewayError(response.status, parsed, "deleteRole");
  }
}
