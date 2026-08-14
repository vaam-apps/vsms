import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  FormField,
  InlineBanner,
  Input,
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
          <FormField
            label="Value (3–11 characters)"
            htmlFor="new-sender-value"
            error={formState.errors.value?.message}
          >
            <Input
              id="new-sender-value"
              aria-invalid={formState.errors.value != null}
              {...register("value")}
            />
          </FormField>
          <FormField label="Kind" htmlFor="new-sender-kind">
            <Input id="new-sender-kind" placeholder="e.g. alphanumeric" {...register("kind")} />
          </FormField>
          <FormField label="Notes (optional)" htmlFor="new-sender-notes">
            <Input id="new-sender-notes" {...register("notes")} />
          </FormField>
          {errorMessage != null && (
            <InlineBanner variant="danger">Create failed: {errorMessage}</InlineBanner>
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
