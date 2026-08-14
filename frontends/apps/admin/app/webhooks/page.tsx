// Server component shell (#55). `endpointId`/`state` filters live in the URL
// via `nuqs`, same reasoning `jobs/page.tsx` gives for its own filters —
// including the `Suspense` boundary `useQueryStates`/`useSearchParams()`
// needs under Next.js. `ADMIN_POLL_INTERVAL_MS` is read server-side (R6,
// AGENTS.md) and handed to the client screen as a prop, matching
// `messages/page.tsx`'s own `MESSAGE_STREAM_POLL_MS` precedent — the
// screen is `"use client"`, and `@vsms/env`'s server vars aren't reachable
// there directly.

import { env } from "@vsms/env";
import { Suspense } from "react";
import { WebhooksScreen } from "./webhooks-screen";

export default function WebhooksPage() {
  return (
    <Suspense fallback={null}>
      <WebhooksScreen attemptsRefetchIntervalMs={env.ADMIN_POLL_INTERVAL_MS} />
    </Suspense>
  );
}
