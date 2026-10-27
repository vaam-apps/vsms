// `consoleKey.*` — the console's own key, as the gateway sees it. See
// `@vsms/gateway`'s `console-keys.ts`. The thumbprint comes from the browser
// (it computed it from its own public key); the gateway's answer is what is
// believed, and it is the caller's own rows only unless the caller is an owner
// or an admin, in which case a thumbprint still selects one row.

import { TRPCError } from "@trpc/server";
import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const thumbprint = z.string().regex(/^[0-9a-f]{64}$/);

export const consoleKeyRouter = router({
  /** 200 when the gateway accepts this browser's key, `UNAUTHORIZED` when it
   *  does not know it (yet). */
  probe: publicProcedure.query(async ({ ctx }) => {
    try {
      await ctx.gateway.probeKey();
      return { paired: true as const };
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  byThumbprint: publicProcedure.input(z.object({ thumbprint })).query(async ({ ctx, input }) => {
    try {
      const key = await ctx.gateway.getConsoleKeyByThumbprint(input.thumbprint);
      if (key === null) throw new TRPCError({ code: "NOT_FOUND", message: "No such key." });
      return key;
    } catch (error) {
      if (error instanceof TRPCError) throw error;
      rethrowGatewayError(error);
    }
  }),

  revoke: publicProcedure
    .input(z.object({ keyId: z.string().min(1) }))
    .mutation(async ({ ctx, input }) => {
      try {
        return await ctx.gateway.revokeConsoleKey(input.keyId);
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),
});
