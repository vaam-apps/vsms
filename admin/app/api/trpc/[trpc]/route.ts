// The Next.js side of Seam 1 (browser <-> Next.js, tRPC). `runtime =
// "nodejs"` is required, not a default left in place: `@vsms/gateway`
// reads mTLS cert/key material from disk (`node:fs`), which the Edge
// runtime cannot do. `dynamic = "force-dynamic"` stops Next from trying
// to statically optimise a route whose whole point is a live upstream
// call — previewMessage/sendMessage responses must never be cached at
// build time.

import { fetchRequestHandler } from "@trpc/server/adapters/fetch";
import { appRouter, createContext } from "@vsms/api";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

function handler(req: Request) {
  return fetchRequestHandler({
    endpoint: "/api/trpc",
    req,
    router: appRouter,
    createContext,
    onError({ error, path }) {
      console.error(`[trpc] ${path ?? "<no-path>"}:`, error);
    },
  });
}

export { handler as GET, handler as POST };
