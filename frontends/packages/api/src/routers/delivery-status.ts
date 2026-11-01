// `deliveryStatus.lookup` — #472's body-free delivery lookup, for the console's
// Delivery lookup screen.
//
// # A read, declared as a mutation, on purpose
//
// The server procedure only reads, but every call that reaches the database
// writes a `DeliveryLookup` row, and a lookup by number or by client reference
// spends the caller's hourly budget. A tRPC *query* is the wrong shape for
// that: react-query retries a query on a network failure or a 5xx (up to three
// times, `@vsms/hooks`' `shouldRetryQuery`) and refetches a stale one when the
// window regains focus, and each of those would spend budget and write an
// audit row nobody asked for. A mutation runs once, when the person submits,
// and is never refetched or retried. `compose.preview` is a query because it
// costs nothing; this is the opposite case.
//
// The input has no `limit`: the server's own default applies, so the console
// holds no page-size value.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const lookupInput = z.object({
  messageId: z.string().min(1).optional(),
  appId: z.string().min(1).optional(),
  clientRef: z.string().min(1).optional(),
  msisdn: z.string().min(1).optional(),
  since: z.string().datetime().optional(),
  until: z.string().datetime().optional(),
});

export const deliveryStatusRouter = router({
  lookup: publicProcedure.input(lookupInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.lookupDeliveryStatus(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
