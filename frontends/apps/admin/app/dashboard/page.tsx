// Server component shell (#49), same shape `jobs/page.tsx`/`workers/page.tsx`
// use. `DashboardScreen` itself needs no `useSearchParams()` (no filters —
// there is nothing to page or filter on a summary screen), so a Suspense
// boundary isn't strictly required here, but the wrapper is kept for
// consistency with every other screen in this app rather than being a
// special case to remember.
//
// `DASHBOARD_REFETCH_INTERVAL_MS` is read here, server-side, and handed
// down as a prop — the same `messages/page.tsx` shape `MESSAGE_STREAM_POLL_MS`
// already uses (AGENTS.md's R6: a tuning value belongs in `@vsms/env`, not
// hardcoded in a `"use client"` screen, which can't read a server-only env
// var directly).

import { env } from "@vsms/env";
import { Suspense } from "react";
import { DashboardScreen } from "./dashboard-screen";

export default function DashboardPage() {
  return (
    <Suspense fallback={null}>
      <DashboardScreen refetchIntervalMs={env.DASHBOARD_REFETCH_INTERVAL_MS} />
    </Suspense>
  );
}
