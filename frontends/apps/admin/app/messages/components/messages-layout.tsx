// Dumb — route-local to messages (R6). The page's own vertical rhythm.
// Pure composition shell: it renders whatever it's handed, in the gap the
// design calls for, and knows nothing about what any of it is.

import type { ReactNode } from "react";

export interface MessagesLayoutProps {
  children: ReactNode;
}

export function MessagesLayout({ children }: MessagesLayoutProps) {
  return <div className="flex flex-col gap-6">{children}</div>;
}
