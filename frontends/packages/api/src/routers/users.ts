import "server-only";

// `users.list` / `users.get` / `users.provision` / `users.update` /
// `users.delete` — #58's Users screen. Thin wraps over `@vsms/gateway`'s
// own `users.ts`.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const provisionInput = z.object({
  email: z.string().email(),
  displayName: z.string().min(1).max(128),
  roleKey: z.string().min(1),
});

const updateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  displayName: z.string().min(1).max(128).optional(),
  roleKey: z.string().min(1).optional(),
  active: z.boolean().optional(),
});

export const usersRouter = router({
  list: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.listUsers();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  get: publicProcedure.input(z.object({ id: z.string().min(1) })).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.getUserById(input.id);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  /**
   * Returns the one-time password exactly once — see
   * `@vsms/gateway/users.ts::provisionUser`'s own doc. No rotate/reset
   * counterpart exists; see `OPEN_QUESTIONS.md`.
   */
  provision: publicProcedure.input(provisionInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.provisionUser(input.email, input.displayName, input.roleKey);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  update: publicProcedure.input(updateInput).mutation(async ({ ctx, input }) => {
    const { id, etag, ...fields } = input;
    try {
      return await ctx.gateway.updateUser(id, etag, fields);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  delete: publicProcedure
    .input(z.object({ id: z.string().min(1) }))
    .mutation(async ({ ctx, input }) => {
      try {
        await ctx.gateway.deleteUser(input.id);
        return { ok: true as const };
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),
});
