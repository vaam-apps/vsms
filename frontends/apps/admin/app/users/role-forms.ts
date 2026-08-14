// Role-form schemas and the client-side echo of the reserved-role-key
// guard, extracted out of `users-screen.tsx` (R6).

import { z } from "zod";

//
// # `RESERVED_ROLE_KEYS`/`ROLE_KEY_PATTERN` — why these stay in code, not
// # `@vsms/env`, and why this is a second, client-safe copy
//
// `@vsms/gateway/roles.ts` already exports `isReservedRoleKey`/
// `isValidRoleKeyShape` — but that whole module is `import "server-only"`,
// and re-exporting through `@vsms/gateway`'s index doesn't strip the
// marker, so importing it here (a `"use client"` screen) fails the build.
// These two functions are trivial, pure regex checks with no security
// weight of their own, so they're duplicated here rather than carving a
// new client-safe shared package out for two one-line functions.
//
// R6's own carve-out test (`AGENTS.md`, the `TXN_TTL_SECONDS` example)
// applies directly: is a wrong value here *inconvenient* or *unsafe*? A
// `Role` keyed literally `system` would let a human read
// `OauthSigningKey.privateKeyPem` (the key that signs every token this
// system issues) and every `UserCredential.passwordHash` through
// generated CRUD — `backends/crates/sms-api/src/auth.rs`'s own doc names this
// exact escalation. That is unsafe, not inconvenient, so this constant is
// not `@vsms/env` material regardless of how trivial it looks.
//
// That said: **this copy is not itself a security boundary.** The two
// real guards are server-side and stay server-side regardless of what
// this file does — a database `CHECK (key NOT IN ('system', 'app'))`
// (`roles_key_not_reserved_check`) and `RESERVED_ROLE_KEYS` inside
// `backends/crates/sms-api/src/auth.rs::load_human_principal`. This third copy exists
// only to turn a mistaken attempt into an immediate, friendly refusal in
// the create form instead of a raw `23514 check_violation` surfacing as an
// unhelpful generic error toast. If this file drifted from the server's
// two guards — a looser regex, a shrunk reserved set — the request would
// still be refused, just later and less helpfully; nothing this console
// does can widen what the server actually allows.
const RESERVED_ROLE_KEYS = new Set(["system", "app"]);
const ROLE_KEY_PATTERN = /^[a-z][a-z0-9_]{2,31}$/;

export function isReservedRoleKey(key: string): boolean {
  return RESERVED_ROLE_KEYS.has(key);
}

export function isValidRoleKeyShape(key: string): boolean {
  return ROLE_KEY_PATTERN.test(key);
}

// # `KNOWN_PERMISSIONS` — a known drift against the server's real
// # vocabulary, reported rather than silently trusted
//
// This list was meant to mirror §5.2's role/permission table, but §5.2's
// table is *descriptive* prose about what each role can do via Layer-1
// `hasRole(...)` checks — it is not the same thing as the literal strings
// `require_permission`/the router's own per-route permission tables
// actually compare a token's `perms`/`scope` claim against. Checked
// directly against `backends/crates/sms-api/src/rbac.rs`, `router.rs` and every
// `require_permission(ctx, "...")` call site in `procedures.rs`, not
// assumed: the permission literals genuinely enforced anywhere today are
// `sms:read`, `sms:send`, `optout:manage`, `webhook:manage`, `job:read`,
// `job:enqueue`, `worker:read`, `dashboard:read`, `audit:read`,
// `user:manage`, `provider:read`, `provider:update`, `route:read`,
// `sender:manage` — fourteen literals. Eight entries below have **no
// corresponding server-side check anywhere**: `message:cancel`
// (`cancelMessage` is still a milestone-2 `not_yet` stub), `app:read`,
// `app:write` (App create/update/delete are Layer-1-only,
// `hasRole('owner') || hasRole('admin')`), `client:provision`,
// `provider:delete`, `route:write` (Route create/update/delete are also
// Layer-1-only), `user:delete`, and `role:manage` (User/Role
// create/update/delete are all `hasRole('owner')`-only, not literal-gated).
// Putting one of those eight in a role's `permissions` field is not wrong
// — the field is free text with no server-side vocabulary check — but it
// creates a false impression that the string does something. Left
// unchanged rather than silently trimmed: fixing the drift means deciding
// whether to add real Layer-2 gates for these eight actions or correct
// §5.2's own table, and that is a server-side RBAC decision, not a console
// styling pass. Flagged here, and in this PR's own report, rather than
// fixed as a drive-by.
export const KNOWN_PERMISSIONS = [
  "sms:read",
  "sms:send",
  "message:cancel",
  "app:read",
  "app:write",
  "client:provision",
  "provider:read",
  "provider:update",
  "provider:delete",
  "route:read",
  "route:write",
  "sender:manage",
  "optout:manage",
  "webhook:manage",
  "job:read",
  "job:enqueue",
  "worker:read",
  "dashboard:read",
  "audit:read",
  "user:manage",
  "user:delete",
  "role:manage",
] as const;

const roleKeySchema = z
  .string()
  .trim()
  .regex(
    ROLE_KEY_PATTERN,
    "Must start with a letter, 3-32 chars, lowercase letters/digits/underscore only",
  )
  .refine((key) => !isReservedRoleKey(key), {
    message: '"system" and "app" are reserved and can never be assigned to a role',
  });

export const roleCreateSchema = z.object({
  key: roleKeySchema,
  label: z.string().trim().min(2, "At least 2 characters").max(64, "At most 64 characters"),
  permissions: z.string(),
});
export type RoleCreateValues = z.infer<typeof roleCreateSchema>;

export const roleEditSchema = z.object({
  label: z.string().trim().min(2, "At least 2 characters").max(64, "At most 64 characters"),
  permissions: z.string(),
});
export type RoleEditValues = z.infer<typeof roleEditSchema>;
