import "server-only";

// `roles.list` / `roles.get` / `roles.create` / `roles.update` /
// `roles.delete` — #58's Roles screen. Thin wraps over `@vsms/gateway`'s
// own `roles.ts`.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const roleKey = z
  .string()
  .regex(/^[a-z][a-z0-9_]{2,31}$/, "lowercase, starts with a letter, 3-32 chars — a-z, 0-9, _")
  .refine((key) => key !== "system" && key !== "app", {
    message: '"system" and "app" are reserved and can never be assigned to a Role',
  });

const createInput = z.object({
  key: roleKey,
  label: z.string().min(2).max(64),
  description: z.string().optional(),
  permissions: z.array(z.string().min(1)),
});

const updateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  label: z.string().min(2).max(64).optional(),
  description: z.string().optional(),
  permissions: z.array(z.string().min(1)).optional(),
});

export const rolesRouter = router({
  list: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.listRoles();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  get: publicProcedure.input(z.object({ id: z.string().min(1) })).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.getRoleById(input.id);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  create: publicProcedure.input(createInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.createRole(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  update: publicProcedure.input(updateInput).mutation(async ({ ctx, input }) => {
    const { id, etag, ...fields } = input;
    try {
      return await ctx.gateway.updateRole(id, etag, fields);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  delete: publicProcedure
    .input(z.object({ id: z.string().min(1) }))
    .mutation(async ({ ctx, input }) => {
      try {
        await ctx.gateway.deleteRole(input.id);
        return { ok: true as const };
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),
});
