import { Input, Label } from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import type { SenderIdFormValues } from "../sender-id-domain";

// Dumb (R6): the sender id's own edit form fields.
export function SenderEditFields({
  formId,
  form,
  onSubmit,
  saveErrorMessage,
}: {
  formId: string;
  form: UseFormReturn<SenderIdFormValues>;
  onSubmit: (values: SenderIdFormValues) => void;
  saveErrorMessage?: string | undefined;
}) {
  const { register, control, formState, handleSubmit } = form;

  return (
    <form id={formId} onSubmit={handleSubmit(onSubmit)} className="flex flex-col gap-4">
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="sender-value">Value</Label>
        <Input
          id="sender-value"
          aria-invalid={formState.errors.value != null}
          {...register("value")}
        />
        {formState.errors.value != null && (
          <p className="text-caption text-state-danger-fg">{formState.errors.value.message}</p>
        )}
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="sender-kind">Kind</Label>
        <Input id="sender-kind" placeholder="e.g. alphanumeric" {...register("kind")} />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="sender-notes">Notes</Label>
        <Input id="sender-notes" {...register("notes")} />
      </div>
      <Controller
        control={control}
        name="active"
        render={({ field }) => (
          <label className="flex items-center gap-2 text-body text-foreground">
            <input
              type="checkbox"
              checked={field.value}
              onChange={(e) => field.onChange(e.target.checked)}
              className="checkbox"
            />
            Active — eligible for <span className="font-mono">sendMessage</span> to resolve as a
            default or explicit sender
          </label>
        )}
      />
      {saveErrorMessage != null && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Save failed: {saveErrorMessage}
        </div>
      )}
    </form>
  );
}
