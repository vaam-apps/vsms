// Dumb view: an inline error banner. Route-local wrapper so call sites in
// this route group say `<ErrorBanner>`, not `<InlineBanner variant="danger">`
// — but the box itself is `@vaam-apps/ui`'s own `InlineBanner`, not a second copy
// of its class string. See `apps-screen.tsx`'s report note: several other
// routes carry an identically-named wrapper for the same reason.

import { InlineBanner } from "@vaam-apps/ui";
import type { ReactNode } from "react";

export function ErrorBanner({ children }: { children: ReactNode }) {
  return <InlineBanner variant="danger">{children}</InlineBanner>;
}
