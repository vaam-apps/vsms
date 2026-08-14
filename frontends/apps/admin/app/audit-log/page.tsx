// Server component shell (#58). `AuditLogScreen` now owns filter/offset
// `nuqs` state, which needs a `Suspense` boundary around it the same shape
// `jobs/page.tsx`/`messages/page.tsx` already establish for their own
// `useQueryStates` calls (Next.js requires this for `useSearchParams()`).
//
// R6: the page size is read server-side from `@vsms/env`'s
// `AUDIT_LOG_PAGE_SIZE` and handed down as a prop — the same shape
// `messages/page.tsx` already establishes for `MESSAGE_STREAM_POLL_MS`.

import { env } from "@vsms/env";
import { Suspense } from "react";
import { AuditLogScreen } from "./audit-log-screen";

export default function AuditLogPage() {
  return (
    <Suspense fallback={null}>
      <AuditLogScreen pageSize={env.AUDIT_LOG_PAGE_SIZE} />
    </Suspense>
  );
}
