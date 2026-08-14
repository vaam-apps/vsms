// Dumb — route-local to the message detail screen (R6). The page's own
// vertical rhythm and max width. Pure composition shell.

import type { ReactNode } from "react";

export interface MessageDetailLayoutProps {
  children: ReactNode;
}

export function MessageDetailLayout({ children }: MessageDetailLayoutProps) {
  return <div className="mx-auto flex w-full max-w-[1000px] flex-col gap-6">{children}</div>;
}
