// Dumb view: an inline error banner. Route-local wrapper — see
// `apps/components/error-banner.tsx`'s own note; the box itself is
// `@vaam-apps/ui`'s `InlineBanner`, not a second copy of its class string.

import { InlineBanner } from "@vaam-apps/ui";
import type { ReactNode } from "react";

export function ErrorBanner({ children }: { children: ReactNode }) {
  return <InlineBanner variant="danger">{children}</InlineBanner>;
}
