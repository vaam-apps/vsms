// `webhookEndpoints.list/get/create/update/remove` and
// `webhookAttempts.list/replay` — #55's Webhooks screen. Thin wraps over
// `@vsms/gateway`'s `webhooks.ts` (see that module's own doc for the
// `eventTypes` packing, the audience, and why writes are real and reachable
// as of #211). There is no secret to create or rotate (VS-8): a delivery is
// sealed under the gateway's own key.

import { GATEWAY_AUDIENCE, GATEWAY_AUDIENCE_MESSAGE, urlRefusal } from "@vsms/gateway";
import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const eventType = z.enum([
  "message.accepted",
  "message.submitted",
  "message.delivered",
  "message.failed",
  "message.expired",
  "message.uncertain",
  "message.cancelled",
]);

// What a delivery is addressed to: the name the receiver's envelope layer is
// built with. Free text of 1-255 characters, as `WebhookEndpoint.audience`, but
// never `vsms` (the gateway's own name), and unique across endpoints, which the
// gateway enforces and answers with a conflict.
const audience = z
  .string()
  .trim()
  .min(1)
  .max(255)
  .refine((v) => v !== GATEWAY_AUDIENCE, { message: GATEWAY_AUDIENCE_MESSAGE });

// A URL the worker can seal a delivery for (`@vsms/gateway`'s `receiverRoute`,
// the rule `sms_webhook::delivery_route` applies): one it cannot would only
// fail at delivery, burn the endpoint's attempts and trip its breaker.
const url = z
  .string()
  .url()
  .superRefine((v, ctx) => {
    const refusal = urlRefusal(v);
    if (refusal !== null) ctx.addIssue({ code: "custom", message: refusal });
  });

const createInput = z.object({
  appId: z.string().min(1),
  url,
  audience,
  eventTypes: z.array(eventType).min(1),
  maskRecipient: z.boolean(),
  maxAttempts: z.number().int().min(1).max(20),
});

const updateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  url: url.optional(),
  audience: audience.optional(),
  eventTypes: z.array(eventType).min(1).optional(),
  maskRecipient: z.boolean().optional(),
  active: z.boolean().optional(),
  maxAttempts: z.number().int().min(1).max(20).optional(),
});

export const webhookEndpointsRouter = router({
  list: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.listWebhookEndpoints();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  get: publicProcedure.input(z.object({ id: z.string().min(1) })).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.getWebhookEndpointById(input.id);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  create: publicProcedure.input(createInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.createWebhookEndpoint(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  update: publicProcedure.input(updateInput).mutation(async ({ ctx, input }) => {
    const { id, etag, ...fields } = input;
    try {
      return await ctx.gateway.updateWebhookEndpoint(id, etag, fields);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  remove: publicProcedure
    .input(z.object({ id: z.string().min(1), etag: z.string().min(1).optional() }))
    .mutation(async ({ ctx, input }) => {
      try {
        await ctx.gateway.deleteWebhookEndpoint(input.id, input.etag);
        return { id: input.id };
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),
});

const attemptState = z.enum(["pending", "delivering", "succeeded", "failed", "dead"]);

const listAttemptsInput = z.object({
  endpointId: z.string().min(1).optional(),
  state: attemptState.optional(),
  limit: z.number().int().positive().max(500).optional(),
  offset: z.number().int().nonnegative().optional(),
});

export const webhookAttemptsRouter = router({
  list: publicProcedure.input(listAttemptsInput).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.listWebhookAttempts(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  replay: publicProcedure
    .input(z.object({ attemptId: z.string().min(1) }))
    .mutation(async ({ ctx, input }) => {
      try {
        return await ctx.gateway.replayWebhookAttempt(input.attemptId);
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),
});
