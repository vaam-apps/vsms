// Dumb component (R6), app-wide: the heading of one section on a screen
// that has more than one list (Webhooks: endpoints, then attempts). An `h2`
// at M3's title-large, an optional muted line, and trailing actions, laid
// out like `PageHeader` one level down.

import type { ReactNode } from "react";

export interface SectionHeaderProps {
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
}

export function SectionHeader({ title, description, actions }: SectionHeaderProps) {
  return (
    <div className="flex flex-col gap-3 pt-4 sm:flex-row sm:items-end sm:justify-between">
      <div className="flex min-w-0 flex-col gap-1">
        <h2 className="text-title-large text-foreground">{title}</h2>
        {description !== undefined && (
          <p className="text-body-medium text-muted-foreground">{description}</p>
        )}
      </div>
      {actions !== undefined && (
        <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>
      )}
    </div>
  );
}
