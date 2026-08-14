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
import { Suspense } from "react";
import { WorkersScreen } from "./workers-screen";

export default function WorkersPage() {
  return (
    <Suspense fallback={null}>
      <WorkersScreen pollMs={env.DIAGNOSTICS_POLL_MS} />
    </Suspense>
  );
}
