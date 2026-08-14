// Route-local (R6): the gallery's own labelled-block scaffold — every
// `*-gallery.tsx` component wraps its demo in one of these. Not promoted to
// `frontends/packages/ui` because it encodes this one screen's own shape
// (a title, an optional description, a vertically stacked body) rather than
// a pattern any other console route has reached for.

import type { ReactNode } from "react";

export function Section({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h2 className="font-medium text-foreground text-title-sm">{title}</h2>
        {description != null && (
          <p className="mt-1 text-caption text-muted-foreground">{description}</p>
        )}
      </div>
      {children}
    </section>
  );
}
