// Dumb view: the "someone else changed this since it loaded" banner.
// Route-local — see `apps/components/error-banner.tsx`'s own note.

import { Button } from "@vsms/ui";

export function StaleWriteBanner({ onReload }: { onReload: () => void }) {
  return (
    <div className="flex items-center justify-between gap-3 rounded-sm border border-state-warning-border bg-state-warning-bg px-3 py-2 text-caption text-state-warning-fg">
      <span>Someone else changed this row since it loaded. Reload to see their edit.</span>
      <Button type="button" variant="secondary" size="sm" onClick={onReload}>
        Reload
      </Button>
    </div>
  );
}
