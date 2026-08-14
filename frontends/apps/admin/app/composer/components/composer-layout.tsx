// Dumb — route-local to the composer (R6). The page's own max width and
// vertical rhythm. Pure composition shell.

import type { ReactNode } from "react";

export interface ComposerLayoutProps {
  children: ReactNode;
}

export function ComposerLayout({ children }: ComposerLayoutProps) {
  return <div className="mx-auto flex w-full max-w-[720px] flex-col gap-8">{children}</div>;
}
