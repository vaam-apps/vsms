// Server component shell (#56), same shape `messages/page.tsx` uses:
// `useQueryStates` needs a Suspense boundary around it (Next.js requires
// this for `useSearchParams()`), so the client screen is wrapped here
// rather than opting the whole route out of static generation.
//
// R6: reads `DIAGNOSTICS_POLL_MS` from `@vsms/env` server-side and hands it
// to the client screen as a prop — `jobs-screen.tsx` is `"use client"` and
// this is a server-only env entry, so it cannot read `env.*` itself
// (`messages/page.tsx` established this exact pattern for
// `MESSAGE_STREAM_POLL_MS`).

import { env } from "@vsms/env";
import { Suspense } from "react";
import { JobsScreen } from "./jobs-screen";

export default function JobsPage() {
  return (
    <Suspense fallback={null}>
      <JobsScreen pollMs={env.DIAGNOSTICS_POLL_MS} />
    </Suspense>
  );
}
