// The Next.js side of Seam 1 (browser <-> Next.js, tRPC). `runtime =
// "nodejs"` is required, not a default left in place: `@vsms/gateway`
// reads mTLS cert/key material from disk (`node:fs`), which the Edge
// runtime cannot do. `dynamic = "force-dynamic"` stops Next from trying
// to statically optimise a route whose whole point is a live upstream
// call — previewMessage/sendMessage responses must never be cached at
// build time.
//
// #211: this is also the ONE true per-request boundary for
// `@vsms/gateway`'s `AsyncLocalStorage`-scoped upstream credential — see
// `packages/gateway/src/request-credential.ts`'s own module doc for why
// that seam exists and why it lives here rather than threaded through
// every tRPC procedure/router individually. `admin/middleware.ts` has
// already run by the time a request reaches this route handler (its own
// `config.matcher` covers `/api/trpc/*`) and has already: gated the
// request on a valid, freshness-checked session (redirecting to `/login`
// otherwise, so this handler never sees a request with no session at
// all), and forwarded that session's own `accessToken` as
// `x-vsms-access-token`. Reading it here and wrapping the whole
// `fetchRequestHandler` call in `runWithRequestCredential` means every
// router (`packages/api/src/routers/*.ts`) and every `@vsms/gateway`
// function it calls gets the signed-in human's own credential by
// construction — no router or gateway function has to ask for it.
//
// The header is expected to always be present here (middleware sets it on
// every request that reaches this far) — its absence is treated as "no
// credential decision made," not as "fall back to the machine credential":
// `runWithRequestCredential(undefined, ...)` makes
// `resolveUpstreamAccessToken()` throw loudly inside whichever gateway
// call first needs it, which is the correct, fail-loud behaviour for a
// state this route should never actually observe.

import { fetchRequestHandler } from "@trpc/server/adapters/fetch";
import { appRouter, createContext } from "@vsms/api";
import { type RequestCredential, runWithRequestCredential } from "@vsms/gateway";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const ACCESS_TOKEN_HEADER = "x-vsms-access-token";

function handler(req: Request) {
  const accessToken = req.headers.get(ACCESS_TOKEN_HEADER);
  const credential: RequestCredential | undefined =
    accessToken === null ? undefined : { kind: "human", accessToken };

  return runWithRequestCredential(credential, () =>
    fetchRequestHandler({
      endpoint: "/api/trpc",
      req,
      router: appRouter,
      createContext,
      onError({ error, path }) {
        console.error(`[trpc] ${path ?? "<no-path>"}:`, error);
      },
    }),
  );
}

export { handler as GET, handler as POST };
