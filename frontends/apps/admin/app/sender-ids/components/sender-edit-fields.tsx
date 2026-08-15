import { FormField, groupLabelId, InlineBanner, Input, RadioGroup } from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import {
  SENDER_ID_KIND_HINTS,
  SENDER_ID_KIND_LABELS,
  SENDER_ID_KINDS,
  type SenderIdFormValues,
} from "../sender-id-domain";

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
      {/* Same RadioGroup as the create dialog. The enum migration converted
          create and missed this one, so a sender created from a closed
          vocabulary could still be *edited* back to free text — which is
          the more dangerous half, since it is the path an operator uses
          repeatedly. */}
      <FormField
        label="Kind"
        htmlFor="sender-kind"
        control="group"
        error={form.formState.errors.kind?.message}
      >
        <Controller
          control={form.control}
          name="kind"
          render={({ field }) => (
            <RadioGroup
              aria-labelledby={groupLabelId("sender-kind")}
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
