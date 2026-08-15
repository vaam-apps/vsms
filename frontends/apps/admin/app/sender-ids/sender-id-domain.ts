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

// Mirrors `SenderIdRegistrationStatus` in schemas/vsms.cstack, which as of
// the enum migration is a real enum backed by a Postgres CHECK constraint.
// Before that this array *was* the only vocabulary anywhere — the column
// was a bare `String` and the database accepted `banana`.
export const KNOWN_STATUSES = ["pending", "submitted", "approved", "rejected"] as const;
export type SenderIdRegistrationStatus = (typeof KNOWN_STATUSES)[number];

// Mirrors `SenderIdKind`. Two values, both words this codebase already
// wrote: `shortcode` came from `vsms-demo-seed` and `send_test_message`,
// which have branched on all-digit values since they were written.
export const SENDER_ID_KINDS = ["alphanumeric", "shortcode"] as const;
export type SenderIdKind = (typeof SENDER_ID_KINDS)[number];

export const SENDER_ID_KIND_LABELS: Record<SenderIdKind, string> = {
  alphanumeric: "Alphanumeric",
  shortcode: "Short code",
};

export const SENDER_ID_KIND_HINTS: Record<SenderIdKind, string> = {
  alphanumeric: "A brand name, 3–11 characters",
  shortcode: "All digits",
};

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
  kind: z.enum(SENDER_ID_KINDS),
  notes: z.string(),
  active: z.boolean(),
});
export type SenderIdFormValues = z.infer<typeof senderIdSchema>;

export const registrationSchema = z.object({
  status: z.enum(KNOWN_STATUSES),
  reference: z.string(),
  rejectionReason: z.string(),
});
export type RegistrationFormValues = z.infer<typeof registrationSchema>;

export const createSenderIdSchema = z.object({
  value: z.string().trim().min(3, "3–11 characters").max(11, "3–11 characters"),
  kind: z.enum(SENDER_ID_KINDS),
  notes: z.string(),
});
export type CreateSenderIdFormValues = z.infer<typeof createSenderIdSchema>;

/**
 * Narrow a `string` from the wire onto the enum.
 *
 * The gateway's generated types still describe these columns as `string`,
 * so the boundary has to narrow somewhere; doing it here means every
 * screen gets the same answer rather than each inventing a cast.
 *
 * The fallback is deliberate and is not defensive padding. Rows written
 * before the enum migration can hold anything the old unconstrained
 * `String` column accepted, and a console that crashes or renders blank on
 * one is worse than one that shows the row and lets an operator correct
 * it. New writes cannot produce an unknown value — the Postgres CHECK
 * refuses them — so this only ever fires on legacy data.
 */
export function asSenderIdKind(raw: string): SenderIdKind {
  return (SENDER_ID_KINDS as readonly string[]).includes(raw)
    ? (raw as SenderIdKind)
    : "alphanumeric";
}

export function asRegistrationStatus(raw: string): SenderIdRegistrationStatus {
  return (KNOWN_STATUSES as readonly string[]).includes(raw)
    ? (raw as SenderIdRegistrationStatus)
    : "pending";
}
