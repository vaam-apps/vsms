import "server-only";

// `senderIds.list/get/create/update` and
// `senderIdRegistrations.list/create/update` — #53's Sender IDs screen. Thin
// wraps over `@vsms/gateway`'s `senders.ts` (see that module's own doc for
// the per-(sender, provider) shape and why writes are real and reachable as
// of #211).

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const senderIdCreateInput = z.object({
  value: z.string().min(3).max(11),
  kind: z.string().min(1),
  notes: z.string().min(1).optional(),
});

const senderIdUpdateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  value: z.string().min(3).max(11).optional(),
  kind: z.string().min(1).optional(),
  // No `.min(1)` here, unlike `senderIdCreateInput.notes` above — an empty
  // string is this screen's own working "clear this field" sentinel over a
  // REST PATCH (`@vsms/gateway/senders.ts`'s own module doc: a real,
  // verified cratestack-macros gap means JSON `null` cannot clear a
  // nullable column on this route, so `""` is what actually works).
  notes: z.string().optional(),
  active: z.boolean().optional(),
});

export const senderIdsRouter = router({
  list: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.listSenderIds();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  get: publicProcedure.input(z.object({ id: z.string().min(1) })).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.getSenderIdById(input.id);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  create: publicProcedure.input(senderIdCreateInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.createSenderId(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  update: publicProcedure.input(senderIdUpdateInput).mutation(async ({ ctx, input }) => {
    const { id, etag, ...fields } = input;
    try {
      return await ctx.gateway.updateSenderId(id, etag, fields);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});

const registrationCreateInput = z.object({
  senderIdId: z.string().min(1),
  providerId: z.string().min(1),
  status: z.string().min(1),
  reference: z.string().min(1).optional(),
});

// Every field optional (a `PATCH` may touch just one). `reference`/
// `rejectionReason` deliberately accept an empty string, not just
// `.min(1)` — that empty string is the actual, working "clear this field"
// value over this REST route; a real `null` silently no-ops instead. See
// `@vsms/gateway/senders.ts`'s own module doc ("a real framework gap") for
// the full, verified reasoning.
const registrationUpdateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  status: z.string().min(1).optional(),
  submittedAt: z.string().min(1).optional(),
  approvedAt: z.string().min(1).optional(),
  reference: z.string().optional(),
  rejectionReason: z.string().optional(),
});

export const senderIdRegistrationsRouter = router({
  list: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.listSenderIdRegistrations();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  create: publicProcedure.input(registrationCreateInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.createSenderIdRegistration(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  update: publicProcedure.input(registrationUpdateInput).mutation(async ({ ctx, input }) => {
    const { id, etag, ...fields } = input;
    try {
      return await ctx.gateway.updateSenderIdRegistration(id, etag, fields);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
