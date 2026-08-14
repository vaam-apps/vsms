// Dumb view: an inline literal reference inside prose — see
// `apps/components/code.tsx`'s own note (identical shape, route-local
// per that pattern rather than lifted into `@vsms/ui`). Every value this
// wraps here (a permission literal, or a provisioned `Role.key`) is not a
// `cs_cuid()`, so `IdDisplay` doesn't apply — `Role.key` is
// `@regex("^[a-z][a-z0-9_]{2,31}$")` in `schemas/vsms.cstack`, not `Cuid`.

import type { ReactNode } from "react";

export function Code({ children }: { children: ReactNode }) {
  return <span className="font-mono text-foreground">{children}</span>;
}
