import {
  Input,
  Label,
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
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="registration-status">Status</Label>
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
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="registration-reference">
          Reference (the provider's own tracking id, if any)
        </Label>
        <Input id="registration-reference" {...register("reference")} />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="registration-rejection-reason">
          Rejection reason (what needs to change before resubmitting)
        </Label>
        <Textarea id="registration-rejection-reason" rows={3} {...register("rejectionReason")} />
      </div>
      {saveErrorMessage != null && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Save failed: {saveErrorMessage}
        </div>
      )}
    </form>
  );
}
