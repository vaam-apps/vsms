"use client";

import type { ReactNode } from "react";
import { Button } from "./button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./dialog";
import { Spinner } from "./spinner";

export interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  /**
   * What will happen, in the user's terms — and for anything
   * irreversible, say so here. "This cannot be undone" is not filler;
   * it is the one sentence that changes the decision.
   */
  description: ReactNode;
  /** Names the action, e.g. "Delete endpoint". Never "OK" or "Yes": the
   * confirm button is often the only text read before clicking, so it has
   * to say what it does on its own. */
  confirmLabel: string;
  cancelLabel?: string;
  /** `destructive` renders the confirm button in the error variant. Use
   * it for anything that deletes, revokes or cancels — not merely for
   * anything important. */
  tone?: "default" | "destructive";
  onConfirm: () => void;
  /** In-flight: both buttons disabled, confirm shows a spinner. The
   * dialog does not close itself — the caller closes it when the write
   * actually lands, so a failure leaves the dialog open with its error
   * rather than vanishing and losing the context. */
  busy?: boolean;
  /** Rendered above the buttons. The place for a failed attempt's message. */
  children?: ReactNode;
}

/**
 * A modal yes/no for an action worth interrupting someone over.
 *
 * # When *not* to use it
 *
 * Most of the time. A modal confirm is an interruption, and an
 * interruption that appears on every delete trains people to dismiss it
 * without reading — at which point it protects nothing and costs a click.
 * `InlineConfirm` (the two-step button that expands in place) is the
 * right default for a row-level action; this is for the ones where
 * getting it wrong is expensive and hard to reverse.
 *
 * # Why the caller closes it
 *
 * `onConfirm` returning does not close the dialog, and `busy` is a prop
 * rather than internal state. Both follow from the same requirement: the
 * dialog must still be open, with its context intact, when a write fails.
 * A self-closing confirm reports failure through a toast on a screen the
 * operator has already been returned to, which is where error messages go
 * to be missed.
 */
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  cancelLabel = "Cancel",
  tone = "default",
  onConfirm,
  busy = false,
  children,
}: ConfirmDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        {children}
        <DialogFooter>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={busy}
            onClick={() => onOpenChange(false)}
          >
            {cancelLabel}
          </Button>
          <Button
            type="button"
            variant={tone === "destructive" ? "destructive" : "primary"}
            size="sm"
            disabled={busy}
            onClick={onConfirm}
          >
            {busy && <Spinner size="xs" />}
            {confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
