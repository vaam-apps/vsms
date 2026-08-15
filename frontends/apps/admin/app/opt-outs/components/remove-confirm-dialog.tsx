// Dumb component (R6): the remove confirmation, moved verbatim out of
// `opt-outs-screen.tsx`.
//
// Known limitation, not fixed here: reachable from inside
// `QuickDetailDrawer`'s own footer — see `jobs/components/requeue-confirm-
// dialog.tsx`'s own doc for the nested-Dialog-inside-an-open-drawer focus
// trap `#274` documented for six confirmations elsewhere in this console.
// This route group wasn't in that audit's scope; flagged, not fixed, for
// the same reason that PR gave for not attempting a primitive-level fix.

import {
  Button,
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  InlineBanner,
} from "@vsms/ui";

export interface RemoveConfirmDialogProps {
  open: boolean;
  pending: boolean;
  errorMessage?: string | undefined;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}

export function RemoveConfirmDialog({
  open,
  pending,
  errorMessage,
  onOpenChange,
  onConfirm,
}: RemoveConfirmDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[440px]">
        <DialogHeader>
          <DialogTitle>Remove this opt-out?</DialogTitle>
        </DialogHeader>
        {errorMessage != null && (
          <InlineBanner variant="danger">Remove failed: {errorMessage}</InlineBanner>
        )}
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" variant="destructive" disabled={pending} onClick={onConfirm}>
            {pending ? "Removing…" : "Remove"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
