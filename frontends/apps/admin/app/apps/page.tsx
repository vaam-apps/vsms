// Server component shell (#52). `AppsScreen` owns a `?panel=<id>` nuqs
// state (docs/design/console-redesign.md §3/D14 — the more-details drawer
// route), which needs a `Suspense` boundary around it the same shape
// `jobs/page.tsx`/`messages/page.tsx` already establish for their own
// `useQueryStates` calls (Next.js requires this for `useSearchParams()`).

import { Suspense } from "react";
import { AppsScreen } from "./apps-screen";

export default function AppsPage() {
  return (
    <Suspense fallback={null}>
      <AppsScreen />
    </Suspense>
  );
}
