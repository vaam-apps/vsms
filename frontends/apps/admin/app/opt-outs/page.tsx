// Server component shell (#58).
//
// R6: `recordOpen`/`deleteConfirmId`/`detailRow` moved from three `useState`
// calls into one grouped `nuqs` `useQueryStates` call in `opt-outs-
// screen.tsx` (R6's own worked "wrong example," verbatim) — that reads
// `useSearchParams()` internally, which Next.js requires a `Suspense`
// boundary around, so this file gained one where it previously had none.

import { RouteSkeleton } from "@vsms/ui";
import { Suspense } from "react";
import { OptOutsScreen } from "./opt-outs-screen";

export default function OptOutsPage() {
  // #308: `fallback={null}` used to sit here — see `RouteSkeleton`'s own
  // doc comment for why that made a slow-to-reveal boundary
  // indistinguishable from a broken one. Mitigation, not a fix for the
  // underlying rAF-gated reveal.
  return (
    <Suspense fallback={<RouteSkeleton />}>
      <OptOutsScreen />
    </Suspense>
  );
}
