// Dumb view: an inline error banner. Route-local rather than shared —
// several other routes carry their own copy of this exact shape too; see
// this PR's own report for why it was kept local rather than lifted into
// `@vsms/ui` (avoiding a shared-file edit while several other routes are
// migrating in parallel).

import type { ReactNode } from "react";

export function ErrorBanner({ children }: { children: ReactNode }) {
  return (
    <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
      {children}
    </div>
  );
}
