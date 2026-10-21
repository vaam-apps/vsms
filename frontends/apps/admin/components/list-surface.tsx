// Dumb component (R6), app-wide: the container every list screen's table
// sits in. One outlined, rounded surface (M3's outlined card at the
// large-increased corner), with an optional toolbar strip above the rows and
// a footer under them for counts and caveats. Before this, each screen drew
// its table straight onto the page background, so the header row, the
// filters and the rows had no edge to belong to.

import type { ReactNode } from "react";

export interface ListSurfaceProps {
  /** Filters, search, a count: anything that acts on the rows below. */
  toolbar?: ReactNode;
  /** One muted line under the rows: a window, a caveat, a total. */
  footer?: ReactNode;
  children: ReactNode;
}

export function ListSurface({ toolbar, footer, children }: ListSurfaceProps) {
  return (
    <section className="relative min-w-0 overflow-hidden rounded-[20px] border border-edge-subtle bg-base-100">
      {toolbar !== undefined && (
        <div className="flex flex-col gap-3 border-edge-subtle border-b bg-surface-container-low px-4 py-3 sm:px-5">
          {toolbar}
        </div>
      )}
      <div className="min-w-0">{children}</div>
      {footer !== undefined && (
        <div className="border-edge-subtle border-t px-4 py-3 text-body-small text-muted-foreground sm:px-5">
          {footer}
        </div>
      )}
    </section>
  );
}
