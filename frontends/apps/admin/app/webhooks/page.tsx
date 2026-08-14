// Server component shell (#55). `endpointId`/`state` filters live in the URL
// via `nuqs`, same reasoning `jobs/page.tsx` gives for its own filters —
// including the `Suspense` boundary `useQueryStates`/`useSearchParams()`
// needs under Next.js.

import { Suspense } from "react";
import { WebhooksScreen } from "./webhooks-screen";

export default function WebhooksPage() {
  return (
    <Suspense fallback={null}>
      <WebhooksScreen />
    </Suspense>
  );
}
