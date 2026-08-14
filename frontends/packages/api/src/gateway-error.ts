import "server-only";

// Shared by every router that calls through `@vsms/gateway`: re-throws a
// `GatewayError` (its own `trpcCode` already computed in
// `@vsms/gateway/errors.ts` from sms-api's real HTTP status) as the
// `TRPCError` it already knows how to be, and lets anything else — a
// thrown network error, a genuine bug — propagate unchanged so tRPC's
// default `INTERNAL_SERVER_ERROR` handling still applies.
//
// Originally written inline in `routers/compose.ts`; factored out once
// `routers/messages.ts` needed the identical logic rather than a second
// copy.

import { TRPCError } from "@trpc/server";
import { GatewayError } from "@vsms/gateway";

export function rethrowGatewayError(error: unknown): never {
  if (error instanceof GatewayError) {
    throw new TRPCError({
      code: error.trpcCode,
      message: error.message,
      cause: error,
    });
  }
  throw error;
}
