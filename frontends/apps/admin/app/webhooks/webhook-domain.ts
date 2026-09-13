// Pure, React-free domain logic for the Webhooks screen (#55, R6): event
// type vocabulary, form schemas, and `payloadFor`, the one small formatter
// that doesn't need a component to exist. The secret-masking formatter
// that used to live here (`maskSecret`) is gone — `secret-field.tsx` now
// renders `@vaam-apps/ui`'s own `MaskedValue`, the component that library
// was written to replace this exact hand-rolled mask with.
// Extracted verbatim out of webhooks-screen.tsx per AGENTS.md's R6 — a
// screen file should read as fetch + handlers + composition.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { z } from "zod";

type RouterOutputs = inferRouterOutputs<AppRouter>;
export type EndpointListItem = RouterOutputs["webhookEndpoints"]["list"][number];
export type AttemptListItem = RouterOutputs["webhookAttempts"]["list"]["items"][number];

export const EVENT_TYPES = [
  "message.accepted",
  "message.submitted",
  "message.delivered",
  "message.failed",
  "message.expired",
  "message.uncertain",
  "message.cancelled",
] as const;
export type EventType = (typeof EVENT_TYPES)[number];

export function payloadFor(attempt: Pick<AttemptListItem, "payload">): string {
  try {
    return JSON.stringify(JSON.parse(attempt.payload), null, 2);
  } catch {
    return attempt.payload;
  }
}

export const endpointSchema = z.object({
  url: z.string().trim().min(1, "URL is required"),
  maxAttempts: z
    .string()
    .trim()
    .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 1 && Number(v) <= 20, {
      message: "1–20",
    }),
  maskRecipient: z.boolean(),
  active: z.boolean(),
});
export type EndpointFormValues = z.infer<typeof endpointSchema>;

export const createEndpointSchema = z.object({
  appId: z.string().trim().min(1, "App id is required"),
  url: z.string().trim().min(1, "URL is required"),
  maxAttempts: z
    .string()
    .trim()
    .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 1 && Number(v) <= 20, {
      message: "1–20",
    }),
  maskRecipient: z.boolean(),
});
export type CreateEndpointFormValues = z.infer<typeof createEndpointSchema>;
