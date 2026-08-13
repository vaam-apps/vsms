import "server-only";

// `auditLog.list` / `auditLog.chainStatus` — #58's read-only Audit log
// screen. Thin wraps over `@vsms/gateway`'s own `audit-log.ts`. No
// mutations at all in this router — see that module's own doc for why
// this is genuinely, not just conventionally, read-only.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const listInput = z.object({
  model: z.string().optional(),
  operation: z.string().optional(),
  actorId: z.string().optional(),
  since: z.string().optional(),
  until: z.string().optional(),
  limit: z.number().int().positive().max(200).optional(),
  offset: z.number().int().nonnegative().optional(),
});

export const auditLogRouter = router({
  list: publicProcedure.input(listInput).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.fetchAuditLog(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  chainStatus: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.fetchAuditChainStatus();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
