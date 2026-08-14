import type { UseFormReturn } from "react-hook-form";
import type { EndpointFormValues, EndpointListItem, EventType } from "../webhook-domain";
import { EndpointEditFields } from "./endpoint-edit-fields";
import { EndpointSecretPanel } from "./endpoint-secret-panel";

// Dumb (R6): the whole more-detail body — secret panel + edit form,
// stacked. A thin composition wrapper so the screen never renders a raw
// `<div className="flex flex-col gap-4">` itself.
export function EndpointMoreDetailBody({
  endpoint,
  justCreatedSecret,
  justRotatedSecret,
  onRotate,
  formId,
  form,
  eventTypes,
  onEventTypesChange,
  onSubmit,
  saveErrorMessage,
}: {
  endpoint: EndpointListItem;
  justCreatedSecret: string | null;
  justRotatedSecret: string | null;
  onRotate: () => void;
  formId: string;
  form: UseFormReturn<EndpointFormValues>;
  eventTypes: EventType[];
  onEventTypesChange: (types: EventType[]) => void;
  onSubmit: (values: EndpointFormValues) => void;
  saveErrorMessage?: string | undefined;
}) {
  return (
    <div className="flex flex-col gap-4">
      <EndpointSecretPanel
        endpoint={endpoint}
        justCreatedSecret={justCreatedSecret}
        justRotatedSecret={justRotatedSecret}
        onRotate={onRotate}
      />
      <EndpointEditFields
        formId={formId}
        form={form}
        eventTypes={eventTypes}
        onEventTypesChange={onEventTypesChange}
        onSubmit={onSubmit}
        saveErrorMessage={saveErrorMessage}
      />
    </div>
  );
}
