import "server-only";

// `workers.locks` — #57's Workers screen. A thin wrap over `@vsms/gateway`'s
// `workerLocks`, no input at all: the underlying procedure always reports
// all six §7.1 roles, and there is nothing to page or filter server-side
// for a six-row snapshot.

import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

export const workersRouter = router({
  locks: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.workerLocks();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
