// Server component shell (#52). `AppsScreen` owns a `?panel=<id>` nuqs
// state (docs/design/console-redesign.md §3/D14 — the more-details drawer
// route), which needs a `Suspense` boundary around it the same shape
// `jobs/page.tsx`/`messages/page.tsx` already establish for their own
// `useQueryStates` calls (Next.js requires this for `useSearchParams()`).

import { RouteSkeleton } from "@vsms/ui";
import { Suspense } from "react";
import { AppsScreen } from "./apps-screen";

export default function AppsPage() {
  // #308: `fallback={null}` used to sit here — see `RouteSkeleton`'s own
  // doc comment for why that made a slow-to-reveal boundary
  // indistinguishable from a broken one. Mitigation, not a fix for the
  // underlying rAF-gated reveal.
  return (
    <Suspense fallback={<RouteSkeleton withFilterBar={false} />}>
      <AppsScreen />
    </Suspense>
  );
}
