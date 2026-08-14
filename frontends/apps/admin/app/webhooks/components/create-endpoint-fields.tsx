import { Input, Label } from "@vsms/ui";
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
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="new-endpoint-app-id">App ID</Label>
        <Input
          id="new-endpoint-app-id"
          placeholder="the App this endpoint belongs to"
          aria-invalid={formState.errors.appId != null}
          {...register("appId")}
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="new-endpoint-url">URL</Label>
        <Input
          id="new-endpoint-url"
          placeholder="https://example.com/webhooks/vsms"
          aria-invalid={formState.errors.url != null}
          {...register("url")}
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label>Event types</Label>
        <EventTypeToggles selected={eventTypes} onChange={onEventTypesChange} />
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="new-endpoint-max-attempts">Max attempts</Label>
          <Input id="new-endpoint-max-attempts" inputMode="numeric" {...register("maxAttempts")} />
        </div>
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
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Create failed: {createErrorMessage}
        </div>
      )}
    </form>
  );
}
