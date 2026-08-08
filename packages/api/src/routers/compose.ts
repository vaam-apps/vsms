import "server-only";

// `compose.preview` / `compose.send` — the exactly-two procedures the
// composer (T13, not built here) needs. `preview` is a tRPC *query*
// (`schema/schema.cstack` declares `procedure previewMessage`, not
// `mutation procedure` — it's synchronous and pure on the Rust side, see
// AGENTS.md's Milestone 0 section); `send` is a tRPC *mutation*, matching
// `mutation procedure sendMessage`.

import { TRPCError } from "@trpc/server";
import { GatewayError } from "@vsms/gateway";
import { z } from "zod";
import { publicProcedure, router } from "../trpc";

const previewInput = z.object({
  body: z.string().min(1),
  to: z.string().optional(),
});

const messageClass = z.enum(["otp", "transactional", "notification", "marketing"]);

const sendInput = z.object({
  to: z.string().min(1),
  body: z.string().min(1),
  senderId: z.string().optional(),
  class: messageClass.optional(),
  clientRef: z.string().optional(),
  scheduledAt: z.string().datetime().optional(),
  validityMinutes: z.number().int().positive().optional(),
});

/** Re-throws a {@link GatewayError} as the `TRPCError` it already knows
 * how to be (`trpcCode` was computed in `@vsms/gateway/errors.ts` from
 * sms-api's real HTTP status); anything else propagates unchanged so
 * tRPC's default `INTERNAL_SERVER_ERROR` handling still applies to
 * genuinely unexpected failures (a thrown network error, a bug). */
function rethrowGatewayError(error: unknown): never {
  if (error instanceof GatewayError) {
    throw new TRPCError({
      code: error.trpcCode,
      message: error.message,
      cause: error,
    });
  }
  throw error;
}

export const composeRouter = router({
  preview: publicProcedure.input(previewInput).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.previewMessage(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  send: publicProcedure.input(sendInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.sendMessage(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
