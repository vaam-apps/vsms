// User-form schemas, extracted out of `users-screen.tsx` (R6). Mirrors
// `packages/api/src/routers/users.ts` — read, not guessed.

import { z } from "zod";

export const provisionUserSchema = z.object({
  email: z.string().trim().email("Enter a valid email address"),
  displayName: z.string().trim().min(1, "Required").max(128, "At most 128 characters"),
  roleKey: z.string().min(1, "Pick a role"),
});
export type ProvisionUserValues = z.infer<typeof provisionUserSchema>;

export const userEditSchema = z.object({
  displayName: z.string().trim().min(1, "Required").max(128, "At most 128 characters"),
  roleKey: z.string().min(1, "Pick a role"),
  active: z.boolean(),
});
export type UserEditValues = z.infer<typeof userEditSchema>;
