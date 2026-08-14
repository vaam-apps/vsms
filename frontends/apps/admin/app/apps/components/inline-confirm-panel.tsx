// Dumb view: an inline destructive-action confirmation, rendered in place
// rather than as a second, nested `Dialog` — see `apps-screen.tsx`'s own
// `ProvisionClientPanel` doc for the live-verified reason (a second focus
// trap nested inside an already-open `MoreDetailDrawer` self-dismisses the
// whole drawer). Reused for both "delete this app" and "retire this
// client".

import { Button } from "@vsms/ui";
import type { ReactNode } from "react";

export function InlineConfirmPanel({
  title,
  description,
  confirmLabel,
  pendingLabel,
  pending,
  onCancel,
  onConfirm,
}: {
  title: string;
  description: ReactNode;
  confirmLabel: string;
  pendingLabel: string;
  pending: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div className="flex flex-col gap-3 rounded-sm border border-state-danger-border bg-state-danger-bg p-4">
      <div>
        <p className="font-medium text-body text-state-danger-fg">{title}</p>
        <p className="mt-1 text-caption text-state-danger-fg">{description}</p>
      </div>
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="button" variant="destructive" disabled={pending} onClick={onConfirm}>
          {pending ? pendingLabel : confirmLabel}
        </Button>
      </div>
    </div>
  );
}
