// Server component shell (#55). `endpointId`/`state` filters live in the URL
// via `nuqs`, same reasoning `jobs/page.tsx` gives for its own filters —
// including the `Suspense` boundary `useQueryStates`/`useSearchParams()`
// needs under Next.js. `DIAGNOSTICS_POLL_MS` is read server-side (R6,
// AGENTS.md) and handed to the client screen as a prop, matching
// `messages/page.tsx`'s own `MESSAGE_STREAM_POLL_MS` precedent — the
// screen is `"use client"`, and `@vsms/env`'s server vars aren't reachable
// there directly. This is the same shared diagnostics-poll cadence
// `jobs/page.tsx`/`workers/page.tsx` already read — `DIAGNOSTICS_POLL_MS`'s
// own module doc in `@vsms/env` anticipated this screen converging onto it
// rather than keeping its own, separately named, same-value env var
// (R6-reconcile).

import { env } from "@vsms/env";
import { Suspense } from "react";
import { WebhooksScreen } from "./webhooks-screen";

export default function WebhooksPage() {
  return (
    <Suspense fallback={null}>
      <WebhooksScreen attemptsRefetchIntervalMs={env.DIAGNOSTICS_POLL_MS} />
    </Suspense>
  );
}
