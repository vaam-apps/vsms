// Dumb view: the screen-level vertical stack — see
// `audit-log/components/audit-log-layout.tsx` for the identical reasoning.

import type { ReactNode } from "react";

export function AppsLayout({ children }: { children: ReactNode }) {
  return <div className="flex flex-col gap-6">{children}</div>;
}
