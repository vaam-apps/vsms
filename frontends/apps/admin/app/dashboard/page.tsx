// Server component shell (#49), same shape `jobs/page.tsx`/`workers/page.tsx`
// use, minus the `<Suspense>` wrapper those need. `DashboardScreen` itself
// needs no `useSearchParams()` (no filters — there is nothing to page or
// filter on a summary screen), so a Suspense boundary isn't required here
// at all.
//
// #308: this file used to wrap `DashboardScreen` in
// `<Suspense fallback={null}>` "for consistency," despite this comment
// already saying the boundary wasn't needed. That unneeded boundary was
// the actual bug: any `<Suspense>` boundary's reveal is
// `requestAnimationFrame`-gated (React's own inline completion script —
// `$RC` schedules the DOM swap via rAF, and the hydration retry that
// follows it schedules a second rAF), and a backgrounded/non-composited
// tab never runs that callback — reproduced directly against this build:
// `document.hidden === true` in the test harness, `<main>` stayed a
// permanently empty placeholder, and manually invoking the exact
// functions React's own script would have called (`window.$RV`, then the
// boundary's `_reactRetry()`) revealed the real, fully-correct content
// instantly. Removing the boundary here removes the dependency on that
// rAF entirely — the content is simply present in the initial HTML, the
// same way every one of the seven screens that never had a `<Suspense>`
// boundary at all already renders. The eight screens that genuinely need
// `<Suspense>` for `useSearchParams()`/`useQueryStates()` can't drop it
// the same way; see each of their own `page.tsx` files for the
// `RouteSkeleton` mitigation instead.
//
// `DASHBOARD_REFETCH_INTERVAL_MS` is read here, server-side, and handed
// down as a prop — the same `messages/page.tsx` shape `MESSAGE_STREAM_POLL_MS`
// already uses (AGENTS.md's R6: a tuning value belongs in `@vsms/env`, not
// hardcoded in a `"use client"` screen, which can't read a server-only env
// var directly).

import { env } from "@vsms/env";
import { DashboardScreen } from "./dashboard-screen";

export default function DashboardPage() {
  return <DashboardScreen refetchIntervalMs={env.DASHBOARD_REFETCH_INTERVAL_MS} />;
}
