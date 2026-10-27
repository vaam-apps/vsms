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
  // The raw 32-byte Ed25519 public key, unpadded base64url: 43 characters.
  publicKey: z
    .string()
    .regex(/^[A-Za-z0-9_-]{43}$/, "the 32-byte public key, base64url, no padding"),
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
   * Makes the client and registers the caller's public key — see
   * `@vsms/gateway/app-clients.ts::provisionClient`'s own doc. Nothing secret
   * is returned: the private half never reached the gateway.
   */
  provision: publicProcedure.input(provisionInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.provisionClient(
        input.appId,
        input.label,
        input.scopes,
        input.publicKey,
      );
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
