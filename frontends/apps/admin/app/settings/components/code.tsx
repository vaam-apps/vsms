// Dumb view: an inline literal reference inside prose — an env var name, a
// source path, or a doc reference quoted in a sentence. Route-local, same
// shape as `apps/components/code.tsx`. `"font-mono text-foreground"`
// repeats 6 times in `settings-panel.tsx` alone; none of the six wraps a
// `cs_cuid()` value, so `IdDisplay` doesn't apply here either — see that
// file's own note.

import type { ReactNode } from "react";

export function Code({ children }: { children: ReactNode }) {
  return <span className="font-mono text-foreground">{children}</span>;
}
