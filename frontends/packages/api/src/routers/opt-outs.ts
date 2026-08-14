import "server-only";

// `optOuts.list` / `optOuts.search` / `optOuts.record` / `optOuts.delete`
// — #58's Opt-outs screen. Thin wraps over `@vsms/gateway`'s own
// `opt-outs.ts`.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const optOutSource = z.enum(["inbound_stop", "admin", "import", "operator"]);

const recordInput = z.object({
  msisdn: z.string().min(1),
  source: optOutSource,
  scope: z.string().min(1).max(64),
  reason: z.string().optional(),
});

export const optOutsRouter = router({
  list: publicProcedure
    .input(z.object({ limit: z.number().int().positive().max(500).optional() }).optional())
    .query(async ({ ctx, input }) => {
      try {
        return await ctx.gateway.listOptOuts(input?.limit);
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),

  search: publicProcedure
    .input(z.object({ msisdn: z.string().min(1) }))
    .query(async ({ ctx, input }) => {
      try {
        return await ctx.gateway.searchOptOutByMsisdn(input.msisdn);
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),

  record: publicProcedure.input(recordInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.recordOptOut(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  delete: publicProcedure
    .input(z.object({ id: z.string().min(1) }))
    .mutation(async ({ ctx, input }) => {
      try {
        await ctx.gateway.deleteOptOut(input.id);
        return { ok: true as const };
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),
});
