// Dumb component (R6), app-wide: every screen's title block.
//
// `@vaam-apps/ui`'s `ScreenHeader` renders the `<h1>` and a clamped
// description, and has no slot for the screen's primary actions, so each
// screen used to place its "New app" / "Record opt-out" button in a row of
// its own under the header, or not at all. This puts the title, the one-line
// description and the actions in one M3 header row: the actions sit at the
// trailing edge from `sm`, and wrap under the title on a phone.
//
// The description is clamped to two lines for the same reason `ScreenHeader`
// clamps it: a header that grows with its prose pushes the content off the
// fold on exactly the screens with the longest prose.

import type { ReactNode } from "react";

export interface PageHeaderProps {
  title: ReactNode;
  description?: ReactNode;
  /** The screen's primary actions, trailing. Buttons, usually one. */
  actions?: ReactNode;
  /** One short line above the title: a record's kind, a back link. */
  eyebrow?: ReactNode;
}

export function PageHeader({ title, description, actions, eyebrow }: PageHeaderProps) {
  return (
    <header className="flex flex-col gap-4 pt-2 sm:flex-row sm:items-end sm:justify-between">
      <div className="flex min-w-0 flex-col gap-1">
        {eyebrow !== undefined && (
          <div className="text-label-large text-muted-foreground">{eyebrow}</div>
        )}
        <h1 className="font-display text-headline-medium text-foreground">{title}</h1>
        {description !== undefined && (
          <p className="max-w-3xl text-body-medium text-muted-foreground">{description}</p>
        )}
      </div>
      {actions !== undefined && (
        <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>
      )}
    </header>
  );
}
