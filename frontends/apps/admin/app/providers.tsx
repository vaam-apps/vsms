"use client";

// Everything the page needs before a screen can ask for data, mounted once:
// the gateway transport (the sealing fetch, signing with the browser's key), the
// router that runs in the page behind tRPC's own wire protocol, and the query
// cache. See `@vsms/gateway`'s `sealing-fetch.ts` and `@vsms/api`'s `index.ts`
// for why none of it is a route handler.

import { Toaster } from "@vaam-apps/ui";
import { appRouter, createContext } from "@vsms/api";
import {
  configureComposeFetch,
  configureGatewayFetch,
  createComposeFetch,
  createSealingFetch,
} from "@vsms/gateway";
import { createLocalFetch, TrpcProvider } from "@vsms/hooks";
import { NuqsAdapter } from "nuqs/adapters/next/app";
import { type ReactNode, useState } from "react";
import type { ConsoleConfig } from "@/lib/console-config";
import { notifyUnauthenticated } from "@/lib/key-events";
import { configureSealer, getRuntime } from "@/lib/sealer";

/** Where the console mounts its pass-through and its composer. */
const GATEWAY_MOUNT = "/api/gateway";
const COMPOSE_MOUNT = "/api/compose";

export function Providers({
  children,
  config,
  queryStaleTimeMs,
}: {
  children: ReactNode;
  /** The gateway's pinned keys and the poll cadence, read server-side in
   * `layout.tsx`. */
  config: ConsoleConfig;
  /** `@vsms/env`'s `QUERY_STALE_TIME_MS`, read server-side in `layout.tsx`. */
  queryStaleTimeMs: number;
}) {
  // Installed once, during the first render and before any child asks for data.
  // Idempotent configuration of module state, not a side effect a re-render
  // could repeat in a way that matters.
  const [localFetch] = useState(() => {
    configureSealer(config);
    configureGatewayFetch(
      createSealingFetch({
        runtime: getRuntime,
        endpoint: GATEWAY_MOUNT,
        onUnauthenticated: notifyUnauthenticated,
      }),
    );
    configureComposeFetch(
      createComposeFetch({
        runtime: getRuntime,
        endpoint: COMPOSE_MOUNT,
        onUnauthenticated: notifyUnauthenticated,
      }),
    );
    return createLocalFetch({
      endpoint: "/api/trpc",
      router: appRouter,
      createContext: () => createContext({ messageStreamPollMs: config.messageStreamPollMs }),
    });
  });

  return (
    <NuqsAdapter>
      <TrpcProvider url="/api/trpc" fetch={localFetch} staleTimeMs={queryStaleTimeMs}>
        {children}
        {/* #54: the Providers/Routes screens are the first consumers of
         * `toast()` — mounted once, here, rather than per-screen (`toast.tsx`'s
         * own doc: "mount once, near the app root"). Placed as a sibling of
         * `children`, not a wrapper — `Toaster` renders a fixed-position
         * overlay with no children of its own. */}
        <Toaster />
      </TrpcProvider>
    </NuqsAdapter>
  );
}
