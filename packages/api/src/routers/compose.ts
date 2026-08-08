import "server-only";

// `compose.preview` / `compose.send` — the exactly-two procedures the
// composer (T13, not built here) needs. `preview` is a tRPC *query*
// (`schema/schema.cstack` declares `procedure previewMessage`, not
// `mutation procedure` — it's synchronous and pure on the Rust side, see
// AGENTS.md's Milestone 0 section); `send` is a tRPC *mutation*, matching
// `mutation procedure sendMessage`.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
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
