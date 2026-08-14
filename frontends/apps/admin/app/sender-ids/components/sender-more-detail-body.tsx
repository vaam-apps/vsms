import type { UseFormReturn } from "react-hook-form";
import type {
  ProviderListItem,
  RegistrationListItem,
  SenderIdFormValues,
} from "../sender-id-domain";
import { RegistrationsList } from "./registrations-list";
import { SenderEditFields } from "./sender-edit-fields";

// Dumb (R6): the whole sender-id more-detail body — edit form +
// registrations list, stacked. A thin composition wrapper so the screen
// never renders a raw `<div className="flex flex-col gap-6">` itself.
export function SenderMoreDetailBody({
  formId,
  form,
  onSubmit,
  saveErrorMessage,
  registrations,
  providerById,
  canRegisterMore,
  onRegisterNew,
  onReviewRegistration,
}: {
  formId: string;
  form: UseFormReturn<SenderIdFormValues>;
  onSubmit: (values: SenderIdFormValues) => void;
  saveErrorMessage?: string | undefined;
  registrations: RegistrationListItem[];
  providerById: Map<string, ProviderListItem>;
  canRegisterMore: boolean;
  onRegisterNew: () => void;
  onReviewRegistration: (registrationId: string) => void;
}) {
  return (
    <div className="flex flex-col gap-6">
      <SenderEditFields
        formId={formId}
        form={form}
        onSubmit={onSubmit}
        saveErrorMessage={saveErrorMessage}
      />
      <RegistrationsList
        registrations={registrations}
        providerById={providerById}
        canRegisterMore={canRegisterMore}
        onRegisterNew={onRegisterNew}
        onReviewRegistration={onReviewRegistration}
      />
    </div>
  );
}
