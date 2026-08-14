// Server component shell (#49), same shape `jobs/page.tsx`/`workers/page.tsx`
// use. `DashboardScreen` itself needs no `useSearchParams()` (no filters —
// there is nothing to page or filter on a summary screen), so a Suspense
// boundary isn't strictly required here, but the wrapper is kept for
// consistency with every other screen in this app rather than being a
// special case to remember.

import { Suspense } from "react";
import { DashboardScreen } from "./dashboard-screen";

export default function DashboardPage() {
  return (
    <Suspense fallback={null}>
      <DashboardScreen />
    </Suspense>
  );
}
