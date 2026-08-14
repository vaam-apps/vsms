import "server-only";

// `providers.list` / `providers.get` / `providers.update` — #54's Providers
// screen. Thin wraps over `@vsms/gateway`'s `listProviders`/
// `getProviderById`/`updateProvider` (see that module's own doc for the
// live grammar findings it inherits from `messages.ts`/`jobs.ts`, and for
// why `update` cannot succeed against a real gateway until #194 lands —
// built and tested regardless, per this repo's own "no dormant code behind
// a flag" convention: this is real code with a documented, honest
// limitation, not a feature hidden until it works).

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const providerState = z.enum(["active", "degraded", "disabled", "draining"]);

const updateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  displayName: z.string().min(2).max(64).optional(),
  state: providerState.optional(),
  maxTps: z.number().positive().optional(),
  maxDailySubmissions: z.number().int().positive().optional(),
  costPerSegmentXaf: z.string().min(1).optional(),
});

export const providersRouter = router({
  list: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.listProviders();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  get: publicProcedure.input(z.object({ id: z.string().min(1) })).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.getProviderById(input.id);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  update: publicProcedure.input(updateInput).mutation(async ({ ctx, input }) => {
    const { id, etag, ...fields } = input;
    try {
      return await ctx.gateway.updateProvider(id, etag, fields);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
