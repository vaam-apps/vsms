import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  FormField,
  groupLabelId,
  InlineBanner,
  Input,
  RadioGroup,
} from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import {
  type CreateSenderIdFormValues,
  SENDER_ID_KIND_HINTS,
  SENDER_ID_KIND_LABELS,
  SENDER_ID_KINDS,
} from "../sender-id-domain";

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
          {/* A radio group, not the free-text input this used to be. `kind`
              was an unconstrained `String` with a placeholder reading
              "e.g. alphanumeric" — so the honest answer to "can I type
              banana here" was yes, all the way to the database. It is a
              real enum now, and there are two values, so showing both
              costs nothing. */}
          <FormField
            label="Kind"
            htmlFor="new-sender-kind"
            control="group"
            error={form.formState.errors.kind?.message}
          >
            <Controller
              control={form.control}
              name="kind"
              render={({ field }) => (
                <RadioGroup
                  aria-labelledby={groupLabelId("new-sender-kind")}
                  value={field.value}
                  onValueChange={field.onChange}
                  options={SENDER_ID_KINDS.map((kind) => ({
                    value: kind,
                    label: SENDER_ID_KIND_LABELS[kind],
                    description: SENDER_ID_KIND_HINTS[kind],
                  }))}
                />
              )}
            />
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
