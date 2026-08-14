// Server component shell (#58). `AuditLogScreen` now owns filter/offset
// `nuqs` state, which needs a `Suspense` boundary around it the same shape
// `jobs/page.tsx`/`messages/page.tsx` already establish for their own
// `useQueryStates` calls (Next.js requires this for `useSearchParams()`).

import { Suspense } from "react";
import { AuditLogScreen } from "./audit-log-screen";

export default function AuditLogPage() {
  return (
    <Suspense fallback={null}>
      <AuditLogScreen />
    </Suspense>
  );
}
