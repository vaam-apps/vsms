// Server component shell (#58).
//
// R6: `recordOpen`/`deleteConfirmId`/`detailRow` moved from three `useState`
// calls into one grouped `nuqs` `useQueryStates` call in `opt-outs-
// screen.tsx` (R6's own worked "wrong example," verbatim) — that reads
// `useSearchParams()` internally, which Next.js requires a `Suspense`
// boundary around, so this file gained one where it previously had none.

import { Suspense } from "react";
import { OptOutsScreen } from "./opt-outs-screen";

export default function OptOutsPage() {
  return (
    <Suspense fallback={null}>
      <OptOutsScreen />
    </Suspense>
  );
}
