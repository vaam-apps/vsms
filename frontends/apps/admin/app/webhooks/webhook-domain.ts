// Pure, React-free domain logic for the Webhooks screen (#55, R6): event
// type vocabulary, form schemas, `payloadFor` and `receiverRoute`, the two
// small formatters that don't need a component to exist. (There is no secret
// to mask any more: VS-8 sealed webhooks under the gateway's own key.)
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

// What a delivery is addressed to (`WebhookEndpoint.audience`, 1-255
// characters): the name the receiver's envelope layer is built with.
const audienceField = z.string().trim().min(1, "Audience is required").max(255, "At most 255");

/**
 * The route a delivery to `url` binds: its path, or `null` when the URL cannot
 * carry a sealed delivery. The same rule as `sms_webhook::delivery_route`
 * (http(s), no credentials, no query, no fragment, a path with no `{`, `}` or
 * `%`), which is what the worker applies at delivery time.
 */
export function receiverRoute(url: string): string | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
  if (parsed.username !== "" || parsed.password !== "") return null;
  if (parsed.search !== "" || parsed.hash !== "") return null;
  if (/[{}%]/.test(parsed.pathname)) return null;
  return parsed.pathname;
}

export const endpointSchema = z.object({
  url: z.string().trim().min(1, "URL is required"),
  audience: audienceField,
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
  audience: audienceField,
  maxAttempts: z
    .string()
    .trim()
    .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 1 && Number(v) <= 20, {
      message: "1–20",
    }),
  maskRecipient: z.boolean(),
});
export type CreateEndpointFormValues = z.infer<typeof createEndpointSchema>;
