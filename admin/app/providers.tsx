"use client";

import { TrpcProvider } from "@vsms/hooks";
import { Toaster } from "@vsms/ui";
import { NuqsAdapter } from "nuqs/adapters/next/app";
import type { ReactNode } from "react";

export function Providers({ children }: { children: ReactNode }) {
  return (
    <NuqsAdapter>
      <TrpcProvider url="/api/trpc">
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
