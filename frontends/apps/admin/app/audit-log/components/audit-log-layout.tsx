// Dumb view: the screen-level vertical stack every section of this route
// renders into. Exists so the smart screen file never needs a raw
// `className`-bearing `<div>` of its own (R6).

import type { ReactNode } from "react";

export function AuditLogLayout({ children }: { children: ReactNode }) {
  return <div className="flex flex-col gap-6">{children}</div>;
}
