// Dumb view: the "someone else changed this since it loaded" banner shown
// on a 412/CONFLICT save. Route-local — see `error-banner.tsx`'s own note.

import { Button } from "@vsms/ui";

export function StaleWriteBanner({ message, onReload }: { message: string; onReload: () => void }) {
  return (
    <div className="flex items-center justify-between gap-3 rounded-sm border border-state-warning-border bg-state-warning-bg px-3 py-2 text-caption text-state-warning-fg">
      <span>{message}</span>
      <Button type="button" variant="secondary" size="sm" onClick={onReload}>
        Reload
      </Button>
    </div>
  );
}
