// Pure, React-free domain logic for the Sender IDs screen (#53, R6): form
// schemas and the registration-summary formatter. Extracted verbatim out of
// sender-ids-screen.tsx per AGENTS.md's R6.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { z } from "zod";

type RouterOutputs = inferRouterOutputs<AppRouter>;
export type SenderIdListItem = RouterOutputs["senderIds"]["list"][number];
export type RegistrationListItem = RouterOutputs["senderIdRegistrations"]["list"][number];
export type ProviderListItem = RouterOutputs["providers"]["list"][number];

export const KNOWN_STATUSES = ["pending", "submitted", "approved", "rejected"] as const;

export function summarizeRegistrations(
  registrations: RegistrationListItem[],
  providerById: Map<string, ProviderListItem>,
): string {
  if (registrations.length === 0) return "not registered anywhere";
  return registrations
    .map((r) => `${providerById.get(r.providerId)?.key ?? r.providerId}: ${r.status}`)
    .join(" · ");
}

export const senderIdSchema = z.object({
  value: z.string().trim().min(3, "3–11 characters").max(11, "3–11 characters"),
  kind: z.string().trim().min(1, "Kind is required"),
  notes: z.string(),
  active: z.boolean(),
});
export type SenderIdFormValues = z.infer<typeof senderIdSchema>;

export const registrationSchema = z.object({
  status: z.string().min(1),
  reference: z.string(),
  rejectionReason: z.string(),
});
export type RegistrationFormValues = z.infer<typeof registrationSchema>;

export const createSenderIdSchema = z.object({
  value: z.string().trim().min(3, "3–11 characters").max(11, "3–11 characters"),
  kind: z.string().trim().min(1, "Kind is required"),
  notes: z.string(),
});
export type CreateSenderIdFormValues = z.infer<typeof createSenderIdSchema>;
