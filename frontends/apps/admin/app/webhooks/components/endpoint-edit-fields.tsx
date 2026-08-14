import { FormField, InlineBanner, Input } from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import type { EndpointFormValues, EventType } from "../webhook-domain";
import { EventTypeToggles } from "./event-type-toggles";

// Dumb (R6): the endpoint edit form's own fields (URL, event types, max
// attempts, mask-recipient/active checkboxes). The secret panel above it
// (`EndpointSecretPanel`) is a separate component — this one owns only
// what `endpointSchema` validates.
export function EndpointEditFields({
  formId,
  form,
  eventTypes,
  onEventTypesChange,
  onSubmit,
  saveErrorMessage,
}: {
  formId: string;
  form: UseFormReturn<EndpointFormValues>;
  eventTypes: EventType[];
  onEventTypesChange: (types: EventType[]) => void;
  onSubmit: (values: EndpointFormValues) => void;
  saveErrorMessage?: string | undefined;
}) {
  const { register, control, formState, handleSubmit } = form;

  return (
    <form id={formId} onSubmit={handleSubmit(onSubmit)} className="flex flex-col gap-4">
      <FormField label="URL" htmlFor="endpoint-url" error={formState.errors.url?.message}>
        <Input id="endpoint-url" aria-invalid={formState.errors.url != null} {...register("url")} />
      </FormField>

      {/* Not a `FormField` — see `create-endpoint-fields.tsx`'s identical
          comment: `EventTypeToggles` is a group of toggle buttons with no
          single `id` a `htmlFor` could name. */}
      <fieldset className="flex flex-col gap-1.5">
        <legend className="font-medium text-body text-foreground">Event types</legend>
        <EventTypeToggles selected={eventTypes} onChange={onEventTypesChange} />
      </fieldset>

      <div className="grid grid-cols-2 gap-3">
        <FormField
          label="Max attempts"
          htmlFor="endpoint-max-attempts"
          error={formState.errors.maxAttempts?.message}
        >
          <Input
            id="endpoint-max-attempts"
            inputMode="numeric"
            aria-invalid={formState.errors.maxAttempts != null}
            {...register("maxAttempts")}
          />
        </FormField>
        <div className="flex flex-col justify-end gap-2 pb-2">
          <Controller
            control={control}
            name="maskRecipient"
            render={({ field }) => (
              <label className="flex items-center gap-2 text-body text-foreground">
                <input
                  type="checkbox"
                  checked={field.value}
                  onChange={(e) => field.onChange(e.target.checked)}
                  className="checkbox"
                />
                Mask recipient MSISDN in payload
              </label>
            )}
          />
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
                Active
              </label>
            )}
          />
        </div>
      </div>

      {saveErrorMessage != null && (
        <InlineBanner variant="danger">Save failed: {saveErrorMessage}</InlineBanner>
      )}
    </form>
  );
}
