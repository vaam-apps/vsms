import { Input, Label } from "@vsms/ui";
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
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="endpoint-url">URL</Label>
        <Input id="endpoint-url" aria-invalid={formState.errors.url != null} {...register("url")} />
        {formState.errors.url != null && (
          <p className="text-caption text-state-danger-fg">{formState.errors.url.message}</p>
        )}
      </div>

      <div className="flex flex-col gap-1.5">
        <Label>Event types</Label>
        <EventTypeToggles selected={eventTypes} onChange={onEventTypesChange} />
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="endpoint-max-attempts">Max attempts</Label>
          <Input
            id="endpoint-max-attempts"
            inputMode="numeric"
            aria-invalid={formState.errors.maxAttempts != null}
            {...register("maxAttempts")}
          />
          {formState.errors.maxAttempts != null && (
            <p className="text-caption text-state-danger-fg">
              {formState.errors.maxAttempts.message}
            </p>
          )}
        </div>
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
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Save failed: {saveErrorMessage}
        </div>
      )}
    </form>
  );
}
