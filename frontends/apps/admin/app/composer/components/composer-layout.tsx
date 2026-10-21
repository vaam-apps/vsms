// Dumb — route-local to the composer (R6). The page's two columns: the
// form on the left and, from `lg`, a sticky handset preview on the right.
// Below `lg` the preview sits under the form, so a phone still sees it
// before the send button scrolls away.

import type { ReactNode } from "react";

export interface ComposerLayoutProps {
  header: ReactNode;
  preview: ReactNode;
  children: ReactNode;
}

export function ComposerLayout({ header, preview, children }: ComposerLayoutProps) {
  return (
    <div className="flex w-full flex-col gap-6">
      {header}
      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_340px] lg:items-start">
        <div className="flex min-w-0 flex-col gap-6 rounded-[24px] border border-edge-subtle bg-base-100 p-5 sm:p-6">
          {children}
        </div>
        <aside className="lg:sticky lg:top-24">{preview}</aside>
      </div>
    </div>
  );
}
