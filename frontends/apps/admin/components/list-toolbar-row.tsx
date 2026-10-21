// Dumb component (R6), app-wide: a list toolbar holding one short note and
// the list's own action, for a tab or section that has an action of its own
// beside the page header's (Users & roles: "Provision user" on one tab,
// "New role" on the other).

import type { ReactNode } from "react";

export function ListToolbarRow({ note, action }: { note: ReactNode; action: ReactNode }) {
  return (
    <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <p className="text-body-small text-muted-foreground">{note}</p>
      <div className="shrink-0">{action}</div>
    </div>
  );
}
