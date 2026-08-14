import "server-only";

// `webhookEndpoints.list/get/create/update/remove/rotateSecret` and
// `webhookAttempts.list/replay` — #55's Webhooks screen. Thin wraps over
// `@vsms/gateway`'s `webhooks.ts` (see that module's own doc for the
// `secret` display decision, the `eventTypes` packing, and why writes are
// real and reachable as of #211).

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

const createInput = z.object({
  appId: z.string().min(1),
  url: z.string().url(),
  eventTypes: z.array(eventType).min(1),
  maskRecipient: z.boolean(),
  maxAttempts: z.number().int().min(1).max(20),
});

const updateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  url: z.string().url().optional(),
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
    .input(z.object({ id: z.string().min(1) }))
    .mutation(async ({ ctx, input }) => {
      try {
        await ctx.gateway.deleteWebhookEndpoint(input.id);
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
  // the same reason `jobs.requeue` needs no `etag` either.
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
