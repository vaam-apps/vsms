// Dumb component (R6, app-wide): a danger-toned inline banner for a failed
// read or write. Three routes (apps, users, simulator) each carried their own
// byte-identical copy, kept apart only to avoid merge collisions between
// parallel screen rewrites; that reason is gone, so there is one now.

import { InlineBanner } from "@vaam-apps/ui";
import type { ReactNode } from "react";

export function ErrorBanner({ children }: { children: ReactNode }) {
  return <InlineBanner variant="danger">{children}</InlineBanner>;
}
