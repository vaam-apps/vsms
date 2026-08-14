import "server-only";

// `appClients.listForApp` / `appClients.provision` / `appClients.retire` /
// `appClients.update` — #52's client-management half of the Apps screen.
// Thin wraps over `@vsms/gateway`'s own `app-clients.ts`.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const provisionInput = z.object({
  appId: z.string().min(1),
  label: z.string().min(1).max(64),
  scopes: z.array(z.string().min(1)).min(1, "at least one scope is required"),
});

const retireInput = z.object({ id: z.string().min(1), etag: z.string().min(1) });

const updateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  label: z.string().min(1).max(64).optional(),
  active: z.boolean().optional(),
});

export const appClientsRouter = router({
  listForApp: publicProcedure
    .input(z.object({ appId: z.string().min(1) }))
    .query(async ({ ctx, input }) => {
      try {
        return await ctx.gateway.listAppClientsForApp(input.appId);
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),

  /**
   * Returns the private key PEM exactly once — see
   * `@vsms/gateway/app-clients.ts::provisionClient`'s own doc. This
   * procedure does nothing further with it; the caller (the composer
   * dialog) is responsible for never round-tripping it through anything
   * longer-lived than the one response.
   */
  provision: publicProcedure.input(provisionInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.provisionClient(input.appId, input.label, input.scopes);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  retire: publicProcedure.input(retireInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.retireAppClient(input.id, input.etag);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  update: publicProcedure.input(updateInput).mutation(async ({ ctx, input }) => {
    const { id, etag, ...fields } = input;
    try {
      return await ctx.gateway.updateAppClient(id, etag, fields);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
