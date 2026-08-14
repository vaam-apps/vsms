import { FormField, InlineBanner, Input } from "@vsms/ui";
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
      <FormField label="Value" htmlFor="sender-value" error={formState.errors.value?.message}>
        <Input
          id="sender-value"
          aria-invalid={formState.errors.value != null}
          {...register("value")}
        />
      </FormField>
      <FormField label="Kind" htmlFor="sender-kind">
        <Input id="sender-kind" placeholder="e.g. alphanumeric" {...register("kind")} />
      </FormField>
      <FormField label="Notes" htmlFor="sender-notes">
        <Input id="sender-notes" {...register("notes")} />
      </FormField>
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
        <InlineBanner variant="danger">Save failed: {saveErrorMessage}</InlineBanner>
      )}
    </form>
  );
}
