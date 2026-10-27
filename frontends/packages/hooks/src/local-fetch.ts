// A `fetch` that answers tRPC's wire protocol in-process.
//
// tRPC's fetch adapter is `Request -> Response` and imports nothing from Node,
// so the router can sit behind it in the page: the link `fetch`es
// `/api/trpc/...`, this hands the request to the adapter, and the adapter runs
// the procedure. Batching, superjson and the error formatter are all tRPC's own,
// exactly as they were when a route handler ran them. What changes is where the
// procedure's code runs: next to the signing key.

import type { AnyRouter } from "@trpc/server";
import { fetchRequestHandler } from "@trpc/server/adapters/fetch";

export interface LocalFetchOptions<TRouter extends AnyRouter> {
  /** The path the link is pointed at, e.g. `"/api/trpc"`. */
  endpoint: string;
  router: TRouter;
  /** The context of one request. */
  createContext: () => TRouter["_def"]["_config"]["$types"]["ctx"];
  onError?: (path: string | undefined, error: unknown) => void;
}

export function createLocalFetch<TRouter extends AnyRouter>(
  options: LocalFetchOptions<TRouter>,
): typeof fetch {
  const { endpoint, router, createContext, onError } = options;
  return (async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const base = typeof window === "undefined" ? "http://localhost" : window.location.origin;
    const request = new Request(
      new URL(typeof input === "string" ? input : String(input), base),
      init,
    );
    return fetchRequestHandler({
      endpoint,
      req: request,
      router,
      createContext,
      onError({ error, path }) {
        onError?.(path, error);
      },
    });
  }) as typeof fetch;
}
