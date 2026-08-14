// Dumb view: an inline error banner. Route-local — see
// `apps/components/error-banner.tsx`'s own note on why this isn't shared.

import type { ReactNode } from "react";

export function ErrorBanner({ children }: { children: ReactNode }) {
  return (
    <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
      {children}
    </div>
  );
}
