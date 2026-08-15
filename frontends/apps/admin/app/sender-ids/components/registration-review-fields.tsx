import { FormField, InlineBanner, Input, RadioGroup, Textarea } from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import { KNOWN_STATUSES, type RegistrationFormValues } from "../sender-id-domain";

// Dumb (R6): the registration-review drawer's own edit form fields.
export function RegistrationReviewFields({
  formId,
  form,
  onSubmit,
  saveErrorMessage,
}: {
  formId: string;
  form: UseFormReturn<RegistrationFormValues>;
  onSubmit: (values: RegistrationFormValues) => void;
  saveErrorMessage?: string | undefined;
}) {
  const { register, control, handleSubmit } = form;

  return (
    <form id={formId} onSubmit={handleSubmit(onSubmit)} className="flex flex-col gap-4">
      {/* A radio group, not a select: four values, and seeing the other
          three is the decision this drawer exists to make. It also cannot
          hit #315's portal-inside-a-focus-trap bug, since `RadioGroup`
          renders inline with no portal and no transition. */}
      <FormField
        label="Status"
        htmlFor="registration-status"
        error={form.formState.errors.status?.message}
      >
        <Controller
          control={control}
          name="status"
          render={({ field }) => (
            <RadioGroup
              aria-label="Registration status"
              value={field.value}
              onValueChange={field.onChange}
              options={KNOWN_STATUSES.map((status) => ({ value: status, label: status }))}
            />
          )}
        />
      </FormField>
      <FormField
        label="Reference (the provider's own tracking id, if any)"
        htmlFor="registration-reference"
      >
        <Input id="registration-reference" {...register("reference")} />
      </FormField>
      <FormField
        label="Rejection reason (what needs to change before resubmitting)"
        htmlFor="registration-rejection-reason"
      >
        <Textarea id="registration-rejection-reason" rows={3} {...register("rejectionReason")} />
      </FormField>
      {saveErrorMessage != null && (
        <InlineBanner variant="danger">Save failed: {saveErrorMessage}</InlineBanner>
      )}
    </form>
  );
}
