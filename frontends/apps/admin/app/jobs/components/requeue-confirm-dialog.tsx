// Dumb component (R6): the requeue confirmation, moved verbatim out of
// `jobs-screen.tsx`. `job` is `null` when closed — the smart screen decides
// when to show one, this file only renders it.
//
// Known limitation, not fixed here: this dialog is reachable from inside
// `QuickDetailDrawer`'s own footer (see `jobs-screen.tsx`'s module doc).
// `#274` documented six other confirmations broken by the identical
// nested-Dialog-inside-an-open-drawer focus trap
// (`admin/app/gallery/page.tsx`'s own repro) but did not audit this route
// group — this one and opt-outs' remove-confirm are very likely a seventh
// and eighth instance, not introduced by this rewrite (the pre-R6 code has
// the same shape). Flagged, not fixed, per that investigation's own
// verdict: no reliable primitive-level fix exists today.

import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@vsms/ui";
import type { JobListItem } from "./jobs-table";

export interface RequeueConfirmDialogProps {
  job: JobListItem | null;
  pending: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}

export function RequeueConfirmDialog({
  job,
  pending,
  onOpenChange,
  onConfirm,
}: RequeueConfirmDialogProps) {
  return (
    <Dialog open={job !== null} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Requeue this job?</DialogTitle>
          <DialogDescription>
            {job != null && (
              <>
                <span className="font-mono text-foreground">{job.kind}</span> failed {job.attempts}{" "}
                time{job.attempts === 1 ? "" : "s"} and is now dead. Requeuing resets its attempts
                counter to 0 and moves it back to pending, where the next{" "}
                <span className="font-mono">jobs</span> poll will pick it up again.
              </>
            )}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" disabled={pending} onClick={onConfirm}>
            Requeue
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
