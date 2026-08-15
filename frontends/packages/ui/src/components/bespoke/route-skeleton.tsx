import { Skeleton } from "../primitives/skeleton";
import { ScreenStack } from "./screen-layout";

// #308's own mitigation half — a real, visible fallback for the
// `<Suspense>` boundary every screen that reads `useSearchParams()`/
// `useQueryStates()` (`nuqs`) must wrap itself in (Next.js requires this;
// see AGENTS.md's own R6-adjacent note on `page.tsx` shells). The eight
// screens that still need that boundary — `messages`, `webhooks`, `jobs`,
// `workers`, `opt-outs`, `apps`, `users`, `audit-log` — used to pass
// `fallback={null}`, which is exactly the shape #308 diagnosed: a
// `<Suspense>` boundary is a real, load-bearing part of Next's streaming
// SSR pipeline, and its reveal (the swap from the fallback to the real
// tree) is `requestAnimationFrame`-gated by both React's own inline
// completion script (`$RC` schedules `$RV` via rAF) and the hydration
// retry that follows it. A backgrounded/non-composited tab — proven live
// against this exact build, see the PR/issue thread — never runs that rAF
// callback, so `fallback={null}` rendered as a permanently empty `<main>`
// with zero signal that anything was ever wrong. This is NOT a fix for
// that mechanism (nothing in application code can make a browser paint a
// hidden tab), it only makes the wait visible instead of indistinguishable
// from broken. Ordinary, foregrounded browser tabs reveal the real
// boundary within one frame regardless, mitigation or not.
//
// One reusable component rather than nine bespoke skeletons — every
// affected screen needs the same shape (a title bar, an optional filter
// row, several table-row-shaped blocks), and `Skeleton` itself already
// forbids shimmer (design doc §3.8/§5.2, see that primitive's own doc)
// so there is no per-screen animation timing to keep in sync anyway. Per
// R6, this lives in `@vsms/ui`, not duplicated per route.

export interface RouteSkeletonProps {
  /** How many table-row-shaped blocks to render. Doesn't need to match the
   * real screen's row height exactly (`Skeleton`'s own doc asks for that
   * only where a skeleton stands in for an already-mounted, known-shape
   * table) — this fallback exists for a `<main>` that hasn't rendered
   * *anything* app-specific yet, not a loading row inside a mounted
   * table. */
  rows?: number;
  /** Whether to render a filter-bar-shaped skeleton row above the table
   * rows — most of the eight screens this serves have one (a search box,
   * a couple of selects); a couple don't. */
  withFilterBar?: boolean;
}

export function RouteSkeleton({ rows = 6, withFilterBar = true }: RouteSkeletonProps) {
  return (
    <ScreenStack>
      {/* Not `ScreenHeader` here: its `title`/`description` slots render
       * inside `<h1>`/`<p>`, and `<p>` cannot contain a block-level
       * element like `Skeleton`'s own `<div>` — the browser's HTML parser
       * gives `<p>` an implied end tag the instant it meets one (no such
       * rule applies to `<h1>`), silently splitting `<p><div/></p>` into
       * two siblings and reparenting the div outside the paragraph. That
       * is a real, reproduced-live hydration-mismatch shape, not a
       * hypothetical — found by rendering this exact component before
       * this comment was written. A plain header block, skeleton bars as
       * direct children, sidesteps it entirely. */}
      <div className="flex flex-col gap-1">
        <Skeleton className="h-7 w-48" />
        <Skeleton className="h-4 w-80" />
      </div>
      {withFilterBar && (
        <div className="flex flex-wrap items-center gap-2">
          <Skeleton className="h-9 w-64" />
          <Skeleton className="h-9 w-32" />
          <Skeleton className="h-9 w-32" />
        </div>
      )}
      <div className="flex flex-col gap-2">
        {Array.from({ length: rows }, (_, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: static placeholder rows, a fixed count, never reordered/removed individually.
          <Skeleton key={i} className="h-11 w-full" />
        ))}
      </div>
    </ScreenStack>
  );
}
