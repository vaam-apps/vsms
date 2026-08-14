import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
} from "@vsms/ui";
import type { UseFormReturn } from "react-hook-form";
import type { CreateSenderIdFormValues } from "../sender-id-domain";

// Dumb (R6): the "New sender ID" dialog, start to finish. Not affected by
// the nested-Dialog-in-drawer bug (see sender-ids-screen.tsx's own module
// doc) — it opens from the toolbar while no drawer is open, so it stays a
// real, centered `Dialog`.
export function CreateSenderDialog({
  open,
  onOpenChange,
  form,
  onSubmit,
  pending,
  errorMessage,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  form: UseFormReturn<CreateSenderIdFormValues>;
  onSubmit: (values: CreateSenderIdFormValues) => void;
  pending: boolean;
  errorMessage?: string | undefined;
}) {
  const { register, formState, handleSubmit } = form;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New sender ID</DialogTitle>
          <DialogDescription>
            Created inactive — activate it from the detail drawer once it's ready to be used.
          </DialogDescription>
        </DialogHeader>
        <form
          id="create-sender-id-form"
          onSubmit={handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="new-sender-value">Value (3–11 characters)</Label>
            <Input
              id="new-sender-value"
              aria-invalid={formState.errors.value != null}
              {...register("value")}
            />
            {formState.errors.value != null && (
              <p className="text-caption text-state-danger-fg">{formState.errors.value.message}</p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="new-sender-kind">Kind</Label>
            <Input id="new-sender-kind" placeholder="e.g. alphanumeric" {...register("kind")} />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="new-sender-notes">Notes (optional)</Label>
            <Input id="new-sender-notes" {...register("notes")} />
          </div>
          {errorMessage != null && (
            <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
              Create failed: {errorMessage}
            </div>
          )}
        </form>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" form="create-sender-id-form" disabled={pending}>
            {pending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
