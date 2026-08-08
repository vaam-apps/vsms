import "server-only";

import { initTRPC } from "@trpc/server";
import { GatewayError } from "@vsms/gateway";
import superjson from "superjson";
import type { Context } from "./context";

// `errorFormatter` is what makes a `GatewayError`'s `fieldErrors` (422 ->
// per-field validation detail, see `@vsms/gateway/errors.ts`) reach the
// browser at all. `routers/compose.ts` re-throws every `GatewayError` as a
// `TRPCError` with `cause: error` — without this formatter that `cause`
// stays server-side only, and `useMutation()`'s `error.data` on the client
// would never carry `fieldErrors` for react-hook-form's `setError` to read.
//
// Always assigning the key (never a conditional spread) keeps the return
// type a single object shape with `fieldErrors: Record<string, string[]> |
// undefined` rather than a union of "has the key" / "doesn't have the key"
// — the latter makes `error.data?.fieldErrors` a compile error on the
// client for the branch that lacks the property at all.
//
// In practice this is almost always `undefined` today — the pinned
// `cratestack-pg =0.5.0` doesn't yet populate `details` on `Validation`
// errors (see `@vsms/gateway/errors.ts::extractFieldErrors`'s own comment)
// — but the plumbing is correct end to end for the day it does, and the
// composer's fallback (an inline banner with the server's message) covers
// the gap until then.
const t = initTRPC.context<Context>().create({
  transformer: superjson,
  errorFormatter({ shape, error }) {
    const fieldErrors = error.cause instanceof GatewayError ? error.cause.fieldErrors : undefined;
    return {
      ...shape,
      data: {
        ...shape.data,
        fieldErrors,
      },
    };
  },
});

export const router = t.router;
export const publicProcedure = t.procedure;
