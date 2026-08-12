import "server-only";

// `routeSimulator.simulate` — #54's route simulator. A thin wrap over
// `@vsms/gateway`'s `simulateRoute`, which calls `POST /$procs/
// simulateRoute` and hands back exactly what the real engine
// (`sms_routing::select_route`) decided — see that module's own doc and
// `crates/sms-api/src/route_simulator.rs`'s for the guarantee that nothing
// in this chain re-implements matching.
//
// A tRPC *query*, not a mutation — `schema.cstack` declares `procedure
// simulateRoute`, not `mutation procedure` (it sends nothing, per its own
// module doc), matching `compose.ts`'s own documented convention: mirror
// whichever the schema declares. `simulator-screen.tsx` still only fires it
// from a form submit (`refetch()`/`enabled: false`), not on mount — a
// `query` here is about tRPC's own vocabulary, not about auto-fetching.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const simulateInput = z.object({
  msisdn: z.string().min(1),
  class: z.enum(["otp", "transactional", "notification", "marketing"]),
  appId: z.string().min(1),
  draw: z.number().min(0).max(1).optional(),
});

export const routeSimulatorRouter = router({
  simulate: publicProcedure.input(simulateInput).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.simulateRoute(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
