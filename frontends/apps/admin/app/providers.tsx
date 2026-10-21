"use client";

import { Toaster } from "@vaam-apps/ui";
import { TrpcProvider } from "@vsms/hooks";
import { NuqsAdapter } from "nuqs/adapters/next/app";
import type { ReactNode } from "react";

export function Providers({
  children,
  queryStaleTimeMs,
}: {
  children: ReactNode;
  /** `@vsms/env`'s `QUERY_STALE_TIME_MS`, read server-side in `layout.tsx`. */
  queryStaleTimeMs: number;
}) {
  return (
    <NuqsAdapter>
      <TrpcProvider url="/api/trpc" staleTimeMs={queryStaleTimeMs}>
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
