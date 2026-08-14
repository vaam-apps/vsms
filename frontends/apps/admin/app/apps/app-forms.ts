// Form schemas and field-shape guards for the Apps screen, extracted out
// of `apps-screen.tsx` so the screen file reads as fetch/handlers/
// composition only (R6). Mirrors `packages/api/src/routers/apps.ts`'s
// `createInput`/`updateInput` — read directly, not guessed.

import { z } from "zod";

// # `SLUG_PATTERN` — why this stays in code, not `@vsms/env`
//
// This mirrors the server's own `App.slug` `@regex` shape. It is not an
// operational tuning value (R6's own carve-out test: is a wrong value
// *inconvenient* or *unsafe*?) — a slug the server would reject either way
// costs the operator one failed submit, nothing more; there is no
// privilege or security consequence to getting the pattern wrong the way
// there is for `RESERVED_ROLE_KEYS` (`users/role-forms.ts`). It stays in
// code for a different reason: it is a copy of a server-side data-shape
// contract, and a copy like that has to change in lockstep with the
// schema it mirrors, not independently at deploy time the way a poll
// interval or a page size can. A deployment variable would let the two
// drift silently; a code constant next to the schema it mirrors at least
// keeps the drift visible in a diff.
const SLUG_PATTERN = /^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$/;

export const appCreateSchema = z.object({
  name: z.string().trim().min(2, "At least 2 characters").max(64, "At most 64 characters"),
  slug: z
    .string()
    .trim()
    .min(2, "At least 2 characters")
    .max(40, "At most 40 characters")
    .regex(SLUG_PATTERN, "lowercase, digits, hyphens — no leading/trailing hyphen"),
  description: z.string().trim().max(500, "At most 500 characters").optional(),
  monthlyQuota: z.number().int("Whole numbers only").nonnegative("Must be zero or more"),
  ipAllowlist: z.string(),
  transliterateToGsm7: z.boolean(),
});
export type AppCreateValues = z.infer<typeof appCreateSchema>;

export const appEditSchema = appCreateSchema.omit({ slug: true }).extend({ active: z.boolean() });
export type AppEditValues = z.infer<typeof appEditSchema>;

export const provisionClientSchema = z.object({
  label: z.string().trim().min(1, "Required").max(64, "At most 64 characters"),
  scopes: z.string().trim().min(1, "At least one scope is required"),
});
export type ProvisionClientValues = z.infer<typeof provisionClientSchema>;

const APP_CREATE_FIELDS = [
  "name",
  "slug",
  "description",
  "monthlyQuota",
  "ipAllowlist",
  "transliterateToGsm7",
] as const;
export function isAppCreateField(field: string): field is (typeof APP_CREATE_FIELDS)[number] {
  return (APP_CREATE_FIELDS as readonly string[]).includes(field);
}

const APP_EDIT_FIELDS = ["name", "description", "monthlyQuota", "ipAllowlist", "active"] as const;
export function isAppEditField(field: string): field is (typeof APP_EDIT_FIELDS)[number] {
  return (APP_EDIT_FIELDS as readonly string[]).includes(field);
}
