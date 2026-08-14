import {
  FormField,
  InlineBanner,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Textarea,
} from "@vsms/ui";
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
      <FormField label="Status" htmlFor="registration-status">
        <Controller
          control={control}
          name="status"
          render={({ field }) => (
            <Select value={field.value} onValueChange={field.onChange}>
              <SelectTrigger id="registration-status">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {KNOWN_STATUSES.map((status) => (
                  <SelectItem key={status} value={status}>
                    {status}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
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
