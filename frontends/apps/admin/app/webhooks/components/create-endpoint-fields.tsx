import { FormField, InlineBanner, Input } from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import type { CreateEndpointFormValues, EventType } from "../webhook-domain";
import { EventTypeToggles } from "./event-type-toggles";

// Dumb (R6): the "New webhook endpoint" dialog's own form fields.
export function CreateEndpointFields({
  formId,
  form,
  eventTypes,
  onEventTypesChange,
  onSubmit,
  createErrorMessage,
}: {
  formId: string;
  form: UseFormReturn<CreateEndpointFormValues>;
  eventTypes: EventType[];
  onEventTypesChange: (types: EventType[]) => void;
  onSubmit: (values: CreateEndpointFormValues) => void;
  createErrorMessage?: string | undefined;
}) {
  const { register, control, formState, handleSubmit } = form;

  return (
    <form id={formId} onSubmit={handleSubmit(onSubmit)} className="flex flex-col gap-4">
      <FormField
        label="App ID"
        htmlFor="new-endpoint-app-id"
        error={formState.errors.appId?.message}
      >
        <Input
          id="new-endpoint-app-id"
          placeholder="the App this endpoint belongs to"
          aria-invalid={formState.errors.appId != null}
          {...register("appId")}
        />
      </FormField>
      <FormField label="URL" htmlFor="new-endpoint-url" error={formState.errors.url?.message}>
        <Input
          id="new-endpoint-url"
          placeholder="https://example.com/webhooks/vsms"
          aria-invalid={formState.errors.url != null}
          {...register("url")}
        />
      </FormField>
      {/* Not a `FormField`: `EventTypeToggles` is a group of toggle buttons,
          not one labelable control, so there is no single `id` for a
          `htmlFor` to name. A `<fieldset>`/`<legend>` instead of an
          orphaned `<label>` with no `for` — the same accessibility gap
          `FormField`'s own `htmlFor` requirement exists to catch, fixed the
          correct way for a group rather than forced through a primitive
          shaped for one control. */}
      <fieldset className="flex flex-col gap-1.5">
        <legend className="font-medium text-body text-foreground">Event types</legend>
        <EventTypeToggles selected={eventTypes} onChange={onEventTypesChange} />
      </fieldset>
      <div className="grid grid-cols-2 gap-3">
        <FormField label="Max attempts" htmlFor="new-endpoint-max-attempts">
          <Input id="new-endpoint-max-attempts" inputMode="numeric" {...register("maxAttempts")} />
        </FormField>
        <div className="flex flex-col justify-end pb-2">
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
                Mask recipient MSISDN
              </label>
            )}
          />
        </div>
      </div>
      {createErrorMessage != null && (
        <InlineBanner variant="danger">Create failed: {createErrorMessage}</InlineBanner>
      )}
    </form>
  );
}
