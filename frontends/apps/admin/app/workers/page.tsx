// Server component shell (#57).
//
// R6: unlike before, this screen now holds one piece of `nuqs` URL state
// (`detail`, the open drawer's role — see `workers-screen.tsx`'s own
// module doc for why it moved out of `useState`), so it needs the same
// `Suspense` boundary `jobs/page.tsx`/`messages/page.tsx` already wrap
// their own `useSearchParams()`-reading screens in. Also reads
// `DIAGNOSTICS_POLL_MS` from `@vsms/env` server-side and hands it down as
// `pollMs`, the same pattern those two pages use.

import { env } from "@vsms/env";
import { RouteSkeleton } from "@vsms/ui";
import { Suspense } from "react";
import { WorkersScreen } from "./workers-screen";

export default function WorkersPage() {
  // #308: `fallback={null}` used to sit here — see `RouteSkeleton`'s own
  // doc comment for why that made a slow-to-reveal boundary
  // indistinguishable from a broken one. Mitigation, not a fix for the
  // underlying rAF-gated reveal.
  return (
    <Suspense fallback={<RouteSkeleton withFilterBar={false} rows={6} />}>
      <WorkersScreen pollMs={env.DIAGNOSTICS_POLL_MS} />
    </Suspense>
  );
}
