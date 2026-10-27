// Pure, React-free domain logic for the Webhooks screen (#55, R6): event
// type vocabulary, form schemas, `payloadFor` and `receiverRoute`, the two
// small formatters that don't need a component to exist. (There is no secret
// to mask any more: VS-8 sealed webhooks under the gateway's own key.)
// Extracted verbatim out of webhooks-screen.tsx per AGENTS.md's R6 — a
// screen file should read as fetch + handlers + composition.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import {
  GATEWAY_AUDIENCE,
  GATEWAY_AUDIENCE_MESSAGE,
  receiverRoute,
  urlRefusal,
} from "@vsms/gateway";
import { z } from "zod";

// Re-exported so the screen's one import line keeps naming the route rule it
// shows ("Your receiver serves POST <path>"); the rule itself lives in
// `@vsms/gateway`, where the tRPC router reads the same one.
export { receiverRoute };

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
// characters, never `vsms`, and unique: the gateway hands each name to one
// endpoint): the name the receiver's envelope layer is built with.
const audienceField = z
  .string()
  .trim()
  .min(1, "Audience is required")
  .max(255, "At most 255")
  .refine((v) => v !== GATEWAY_AUDIENCE, { message: GATEWAY_AUDIENCE_MESSAGE });

// The URL a delivery is POSTed to. One the worker could not seal for (see
// `receiverRoute`) is refused here, as the gateway refuses it on write, instead
// of failing at delivery.
const urlField = z
  .string()
  .trim()
  .min(1, "URL is required")
  .superRefine((v, ctx) => {
    const refusal = urlRefusal(v);
    if (refusal !== null) ctx.addIssue({ code: "custom", message: refusal });
  });

export const endpointSchema = z.object({
  url: urlField,
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
  url: urlField,
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
