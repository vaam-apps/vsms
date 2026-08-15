import "server-only";

// `routes.list` / `routes.get` / `routes.create` / `routes.update` /
// `routes.remove` — #54's Routes screen. Thin wraps over `@vsms/gateway`'s
// `listRoutes`/`getRouteById`/`createRoute`/`updateRoute`/`deleteRoute` —
// see that module's own doc for why every write here is real, tested code
// that is reachable against a real gateway as of #211 (a signed-in
// `owner`/`admin` session succeeds; anyone else gets a real 403).

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const operatorCode = z.enum(["mtn", "orange", "camtel", "nexttel", "unknown"]);
const messageClass = z.enum(["otp", "transactional", "notification", "marketing"]);

const createInput = z.object({
  name: z.string().min(2).max(64),
  priority: z.number().int().min(0).max(1000),
  weight: z.number().int().min(0).max(1000),
  enabled: z.boolean(),
  matchOperator: operatorCode.optional(),
  matchClass: messageClass.optional(),
  matchAppId: z.string().min(1).optional(),
  matchPrefix: z.string().min(1).optional(),
  providerId: z.string().min(1),
  failoverRouteId: z.string().min(1).optional(),
});

// Every field optional (a `PATCH` may touch just one), spelled out
// explicitly rather than derived from `createInput` via a computed
// `Object.fromEntries` — that would type each field as a loose union
// instead of the specific optional zod type each one actually is, which
// would then fail to satisfy `@vsms/gateway`'s own `UpdateRouteFields`
// (`Partial<CreateRouteFields>`) at the call site below.
const updateInput = z.object({
  id: z.string().min(1),
  etag: z.string().min(1),
  name: z.string().min(2).max(64).optional(),
  priority: z.number().int().min(0).max(1000).optional(),
  weight: z.number().int().min(0).max(1000).optional(),
  enabled: z.boolean().optional(),
  matchOperator: operatorCode.optional(),
  matchClass: messageClass.optional(),
  matchAppId: z.string().min(1).optional(),
  matchPrefix: z.string().min(1).optional(),
  providerId: z.string().min(1).optional(),
  failoverRouteId: z.string().min(1).optional(),
});

export const routesRouter = router({
  list: publicProcedure.query(async ({ ctx }) => {
    try {
      return await ctx.gateway.listRoutes();
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  get: publicProcedure.input(z.object({ id: z.string().min(1) })).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.getRouteById(input.id);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  create: publicProcedure.input(createInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.createRoute(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  update: publicProcedure.input(updateInput).mutation(async ({ ctx, input }) => {
    const { id, etag, ...fields } = input;
    try {
      return await ctx.gateway.updateRoute(id, etag, fields);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  remove: publicProcedure
    .input(z.object({ id: z.string().min(1), etag: z.string().min(1).optional() }))
    .mutation(async ({ ctx, input }) => {
      try {
        await ctx.gateway.deleteRoute(input.id, input.etag);
        return { id: input.id };
      } catch (error) {
        rethrowGatewayError(error);
      }
    }),
});
