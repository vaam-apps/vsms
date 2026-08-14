// Dumb view: an inline literal reference inside prose — a permission
// string, role key, or provisioned value quoted in a sentence. Route-local:
// `"font-mono text-foreground"` repeats 4 times in `apps-header.tsx` alone
// and once more in `provision-client-panel-view.tsx`.
//
// Not `IdDisplay` — that component's own contract (§7.3) is specifically
// for `cs_cuid()` values: no middle ellipsis, no prefix, always copyable.
// Every value this wraps here is either static prose (a permission literal
// like `app:write`) or a schema `String` that is not a `Cuid`
// (`AppClient.clientId` is `@length(min: 8, max: 64)`, not `Cuid` —
// checked against `schemas/vsms.cstack` before assuming otherwise) — using
// `IdDisplay`'s truncate-and-copy affordance on either would be wrong.

import type { ReactNode } from "react";

export function Code({ children }: { children: ReactNode }) {
  return <span className="font-mono text-foreground">{children}</span>;
}
