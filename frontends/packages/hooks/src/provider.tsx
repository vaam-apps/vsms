"use client";

// The tRPC + TanStack Query provider, mounted once in
// `frontends/apps/admin/app/providers.tsx`.
//
// **The router runs in the page, not behind a route.** `fetch` below is the
// transport the link uses; the console gives it one that hands the request to
// tRPC's own fetch adapter in-process (`createLocalFetch`), so a procedure runs
// in the browser, where the signing key is, and reaches the gateway through the
// sealing transport (`@vsms/gateway`). The protocol on this side of that is
// still tRPC's: batching, superjson, and the error formatter that carries a
// validation error's `fieldErrors`. Nothing in this package knows about keys.
//
// **Polling, not streaming — permanently, per the DECISIONS section of
// the architecture plan.** The plan's own §4/§5 draft called for
// `splitLink` to route subscriptions over `httpSubscriptionLink` (SSE)
// and everything else over `httpBatchStreamLink`. The owner's decision
// (2026-08-08) drops real streaming entirely: no `LISTEN`/`NOTIFY`, no
// hand-written SSE route, no `httpSubscriptionLink`. `messages.onStateChange`
// is a poll dressed as an `AsyncIterable`, at ~1s median latency — "polling
// with a streaming interface," never described as streaming anywhere in this
// codebase. Accordingly this provider only ever needs one link, and that stays
// true: a poll loop is driven by `setInterval`, not by a persistent HTTP
// subscription.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { httpBatchStreamLink } from "@trpc/client";
import { type ReactNode, useState } from "react";
import superjson from "superjson";
import { shouldRetryQuery } from "./retry";
import { trpc } from "./trpc";

export interface TrpcProviderProps {
  children: ReactNode;
  /** The endpoint the link names, e.g. `"/api/trpc"`. Nothing listens there:
   * `fetch` answers it. */
  url: string;
  /** The transport: in the console, `createLocalFetch(...)`. */
  fetch: typeof fetch;
  /** How long a result counts as fresh (`@vsms/env`'s `QUERY_STALE_TIME_MS`).
   * TanStack's own default is 0: every remount and every window focus
   * refetched every query on the screen, so returning to a screen always
   * started a round trip, and tabbing back to the console re-fired every
   * list at once. Polled queries keep their own `refetchInterval`. */
  staleTimeMs: number;
}

export function TrpcProvider({ children, url, fetch, staleTimeMs }: TrpcProviderProps) {
  const [queryClient] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: { staleTime: staleTimeMs, retry: shouldRetryQuery },
        },
      }),
  );
  const [trpcClient] = useState(() =>
    trpc.createClient({
      links: [
        httpBatchStreamLink({
          url,
          // tRPC's `FetchEsque` is narrower than `fetch` on `RequestInit.signal`
          // under `exactOptionalPropertyTypes`; the function is the same.
          fetch: fetch as unknown as NonNullable<
            Parameters<typeof httpBatchStreamLink>[0]["fetch"]
          >,
          transformer: superjson,
        }),
      ],
    }),
  );

  return (
    <trpc.Provider client={trpcClient} queryClient={queryClient}>
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    </trpc.Provider>
  );
}
