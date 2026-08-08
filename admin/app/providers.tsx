"use client";

import { TrpcProvider } from "@vsms/hooks";
import type { ReactNode } from "react";

export function Providers({ children }: { children: ReactNode }) {
  return <TrpcProvider url="/api/trpc">{children}</TrpcProvider>;
}
