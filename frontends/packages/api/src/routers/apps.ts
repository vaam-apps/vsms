import "server-only";

// `apps.list` / `apps.get` / `apps.create` / `apps.update` / `apps.delete`
// — #52's Apps screen. Thin wraps over `@vsms/gateway`'s own `apps.ts`.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const createInput = z.object({
  name: z.string().min(2).max(64),
  slug: z
    .string()
    .min(2)
    .max(40)
    .regex(
      /^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$/,
      "lowercase, digits, hyphens — no leading/trailing hyphen",
    ),
  description: z.string().optional(),
  monthlyQuota: z.number().int().nonnegative(),
  ipAllowlist: z.array(z.string().min(1)).default([]),
  transliterateToGsm7: z.boolean(),
});

const updateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  name: z.string().min(2).max(64).optional(),
  description: z.string().optional(),
  monthlyQuota: z.number().int().nonnegative().optional(),
  ipAllowlist: z.array(z.string().min(1)).optional(),
  transliterateToGsm7: z.boolean().optional(),
  active: z.boolean().optional(),
});

export const appsRouter = router({
  list: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.listApps();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  get: publicProcedure.input(z.object({ id: z.string().min(1) })).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.getAppById(input.id);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  create: publicProcedure.input(createInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.createApp({
        ...input,
        ipAllowlist: ctx.gateway.packIpAllowlist(input.ipAllowlist),
      });
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  update: publicProcedure.input(updateInput).mutation(async ({ ctx, input }) => {
    const { id, etag, ipAllowlist, ...rest } = input;
    try {
      return await ctx.gateway.updateApp(id, etag, {
        ...rest,
        ipAllowlist:
          ipAllowlist === undefined ? undefined : ctx.gateway.packIpAllowlist(ipAllowlist),
      });
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  delete: publicProcedure
    .input(z.object({ id: z.string().min(1) }))
    .mutation(async ({ ctx, input }) => {
      try {
        await ctx.gateway.deleteApp(input.id);
        return { ok: true as const };
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),
});
