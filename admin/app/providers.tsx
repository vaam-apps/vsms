"use client";

import { TrpcProvider } from "@vsms/hooks";
import { NuqsAdapter } from "nuqs/adapters/next/app";
import type { ReactNode } from "react";

export function Providers({ children }: { children: ReactNode }) {
  return (
    <NuqsAdapter>
      <TrpcProvider url="/api/trpc">{children}</TrpcProvider>
    </NuqsAdapter>
  );
}
