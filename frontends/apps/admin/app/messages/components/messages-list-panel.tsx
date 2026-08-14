// Dumb — route-local to messages (R6). Positions the sticky "N new
// messages" pill relative to the table beneath it.

import type { ReactNode } from "react";

export interface MessagesListPanelProps {
  children: ReactNode;
}

export function MessagesListPanel({ children }: MessagesListPanelProps) {
  return <div className="relative">{children}</div>;
}
