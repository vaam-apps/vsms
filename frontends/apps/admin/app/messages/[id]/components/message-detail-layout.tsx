// Dumb component (R6): the message detail page's two columns. On a wide
// screen the record (body and fields) reads on the left and its history
// (timeline) runs down the right, so the "what is it" and "what happened to
// it" questions sit side by side instead of one card under another; below
// `lg` they stack in that order. Receipts take the full width underneath.

import type { ReactNode } from "react";

export interface MessageDetailLayoutProps {
  header: ReactNode;
  main: ReactNode;
  aside?: ReactNode;
  footer?: ReactNode;
}

export function MessageDetailLayout({ header, main, aside, footer }: MessageDetailLayoutProps) {
  return (
    <div className="flex flex-col gap-6">
      {header}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <div className="flex min-w-0 flex-col gap-4">{main}</div>
        {aside !== undefined && <div className="flex min-w-0 flex-col gap-4">{aside}</div>}
      </div>
      {footer}
    </div>
  );
}
