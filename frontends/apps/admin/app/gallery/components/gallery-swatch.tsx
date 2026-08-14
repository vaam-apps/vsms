// Route-local (R6): a labelled demo row — an eyebrow-style caption above
// whatever the swatch is demonstrating. Not promoted to `@vsms/ui`, same
// reasoning `Section` in this same directory already gives: this is the
// gallery's own scaffold, not a pattern any real screen reaches for.
//
// Extracted because the caption alone — `"mb-2 text-micro
// text-subtle-foreground tracking-[0.03em]"` — was duplicated verbatim
// **9 times** across `data-display-gallery.tsx`, `status-pill-gallery.tsx`,
// and `job-and-attempt-pill-gallery.tsx`. The row beneath the caption is
// deliberately *not* folded in alongside it: those nine rows disagree on
// their own layout (`gap-3` vs `gap-6`, `items-center` present or not, and
// one is a bordered box rather than a flex row at all) — baking one shape
// in would either silently change four of those rows' spacing or need an
// escape hatch that defeats the point. `label` is the one thing every call
// site actually agreed on.

import type { ReactNode } from "react";

export interface GallerySwatchProps {
  label: ReactNode;
  children: ReactNode;
}

export function GallerySwatch({ label, children }: GallerySwatchProps) {
  return (
    <div>
      <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">{label}</p>
      {children}
    </div>
  );
}
