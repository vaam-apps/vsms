import "server-only";

// `dashboard.summary` — #49's Dashboard screen. A thin wrap over
// `@vsms/gateway`'s `dashboardSummary`, no input at all: the underlying
// procedure always reports one snapshot for the caller's own scope (see
// `crates/sms-api/src/procedures.rs`'s own doc on `dashboard_snapshot`),
// nothing to page or filter server-side.

import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

export const dashboardRouter = router({
  summary: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.dashboardSummary();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
