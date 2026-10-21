// Dumb component (R6), app-wide: one muted line of context under a list or
// a grid — a caveat an operator should be able to find, but that should not
// sit above the data the way a banner does.

import type { ReactNode } from "react";

export function Caption({ children }: { children: ReactNode }) {
  return <p className="max-w-3xl text-body-small text-muted-foreground">{children}</p>;
}
