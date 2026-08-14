"use client";

// The tRPC + TanStack Query provider, mounted once in
// `frontends/apps/admin/app/providers.tsx`.
//
// **Polling, not streaming — permanently, per the DECISIONS section of
// the architecture plan.** The plan's own §4/§5 draft called for
// `splitLink` to route subscriptions over `httpSubscriptionLink` (SSE)
// and everything else over `httpBatchStreamLink`. The owner's decision
// (2026-08-08) drops real streaming entirely: no `LISTEN`/`NOTIFY`, no
// hand-written SSE route, no `httpSubscriptionLink`. `messages.onStateChange`
// (T10, not built here) will be a poll dressed as an `AsyncIterable`, at
// ~1s median latency — "polling with a streaming interface," never
// described as streaming anywhere in this codebase. Accordingly this
// provider only ever needs one link, `httpBatchStreamLink`, and that stays
// true even once T10 lands: a poll loop is driven by `setInterval`, not by
// a persistent HTTP subscription.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { httpBatchStreamLink } from "@trpc/client";
import { type ReactNode, useState } from "react";
import superjson from "superjson";
import { trpc } from "./trpc";

export interface TrpcProviderProps {
  children: ReactNode;
  /** The Next.js tRPC route handler's own endpoint, e.g. `"/api/trpc"`. */
  url: string;
}

export function TrpcProvider({ children, url }: TrpcProviderProps) {
  const [queryClient] = useState(() => new QueryClient());
  const [trpcClient] = useState(() =>
    trpc.createClient({
      links: [
        httpBatchStreamLink({
          url,
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
