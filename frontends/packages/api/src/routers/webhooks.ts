// `webhookEndpoints.list/get/create/update/remove/rotateSecret` and
// `webhookAttempts.list/replay` — #55's Webhooks screen. Thin wraps over
// `@vsms/gateway`'s `webhooks.ts` (see that module's own doc for the
// `eventTypes` packing, the two delivery modes, the `secret` display decision,
// and why writes are real and reachable as of #211). An endpoint is `sealed`
// (an audience, no secret) or `hmac` (a secret, no audience), which the create
// input says with a discriminated union so a mismatch is refused here, before
// the gateway's CHECKs would answer it with a database error.

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

const createCommon = {
  appId: z.string().min(1),
  url,
  eventTypes: z.array(eventType).min(1),
  maskRecipient: z.boolean(),
  maxAttempts: z.number().int().min(1).max(20),
};

// `sealed` names the audience its receiver answers to. `hmac` has none: the
// object is strict, so an audience sent with it is an error, not a value the
// database would quietly keep unused.
const createInput = z.discriminatedUnion("deliveryMode", [
  z.object({ deliveryMode: z.literal("sealed"), audience, ...createCommon }).strict(),
  z.object({ deliveryMode: z.literal("hmac"), ...createCommon }).strict(),
]);

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

  // Deliberately its own top-level mutation, not folded into `update` —
  // rotation is a materially different, more consequential action (it
  // starts the `prevSecret` overlap clock) and goes through the dedicated
  // `rotateWebhookSecret` procedure server-side, not a plain field PATCH.
  // No `etag`/`If-Match` here: the procedure reads and writes the row
  // itself under `@isolation("serializable")` (`procedures.rs`'s own doc),
  // the same reason `jobs.requeue` needs no `etag` either. Only an `hmac`
  // endpoint has a secret: the server refuses a sealed one with a 422.
  rotateSecret: publicProcedure
    .input(z.object({ endpointId: z.string().min(1) }))
    .mutation(async ({ ctx, input }) => {
      try {
        return await ctx.gateway.rotateWebhookSecret(input.endpointId);
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
