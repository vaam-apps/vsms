// Pure, React-free domain logic for the Webhooks screen (#55, R6): event
// type vocabulary, the two delivery modes, form schemas, `payloadFor` and
// `receiverRoute`, the small formatters that don't need a component to exist.
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

// How an endpoint's deliveries are authenticated (`WebhookEndpoint.deliveryMode`,
// VS-8R). `sealed` is the recommended one; `hmac` is the legacy scheme a
// deployment will later be able to switch off (VSA-1).
export const DELIVERY_MODES = ["sealed", "hmac"] as const;
export type DeliveryMode = (typeof DELIVERY_MODES)[number];

export const DELIVERY_MODE_LABELS: Record<DeliveryMode, string> = {
  sealed: "Sealed (recommended)",
  hmac: "HMAC signature",
};

export const DELIVERY_MODE_HINTS: Record<DeliveryMode, string> = {
  sealed:
    "Signed with the gateway's own key. Your receiver holds nothing secret: it pins the gateway's public keys and answers to an audience you name.",
  hmac: "The original X-Sms-Signature, signed with a secret shared with your receiver. For a receiver that has not moved to sealed deliveries yet.",
};

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

// A `sealed` endpoint needs an audience, valid as the column holds it. An `hmac`
// endpoint has none: whatever the (hidden) field holds is ignored, and the
// screen never sends it.
function sealedNeedsAudience(
  value: { deliveryMode: DeliveryMode; audience: string },
  ctx: z.RefinementCtx,
) {
  if (value.deliveryMode !== "sealed") return;
  const parsed = audienceField.safeParse(value.audience);
  if (parsed.success) return;
  for (const issue of parsed.error.issues) {
    ctx.addIssue({ code: "custom", path: ["audience"], message: issue.message });
  }
}

const deliveryModeField = z.enum(DELIVERY_MODES);

export const endpointSchema = z
  .object({
    deliveryMode: deliveryModeField,
    url: urlField,
    audience: z.string(),
    maxAttempts: z
      .string()
      .trim()
      .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 1 && Number(v) <= 20, {
        message: "1–20",
      }),
    maskRecipient: z.boolean(),
    active: z.boolean(),
  })
  .superRefine(sealedNeedsAudience);
export type EndpointFormValues = z.infer<typeof endpointSchema>;

export const createEndpointSchema = z
  .object({
    appId: z.string().trim().min(1, "App id is required"),
    deliveryMode: deliveryModeField,
    url: urlField,
    audience: z.string(),
    maxAttempts: z
      .string()
      .trim()
      .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 1 && Number(v) <= 20, {
        message: "1–20",
      }),
    maskRecipient: z.boolean(),
  })
  .superRefine(sealedNeedsAudience);
export type CreateEndpointFormValues = z.infer<typeof createEndpointSchema>;
