import { Button, FieldError, InlineEmptyState } from "@vsms/ui";
import type { ProviderListItem, RegistrationListItem } from "../sender-id-domain";
import { RegistrationStatusBadge } from "./registration-status-badge";

// Dumb (R6): the full per-provider registrations table inside the sender
// id's more-detail drawer, plus its own "Register with a new provider"
// action.
export function RegistrationsList({
  registrations,
  providerById,
  canRegisterMore,
  onRegisterNew,
  onReviewRegistration,
}: {
  registrations: RegistrationListItem[];
  providerById: Map<string, ProviderListItem>;
  canRegisterMore: boolean;
  onRegisterNew: () => void;
  onReviewRegistration: (registrationId: string) => void;
}) {
  return (
    <div className="flex flex-col gap-3 border-edge border-t pt-4">
      <div className="flex items-center justify-between">
        <h3 className="font-medium text-body text-foreground">Registrations, per provider</h3>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          disabled={!canRegisterMore}
          onClick={onRegisterNew}
        >
          Register with a new provider
        </Button>
      </div>

      {registrations.length === 0 ? (
        <InlineEmptyState message="Not registered with any provider yet." />
      ) : (
        <div className="flex flex-col gap-2">
          {registrations.map((registration) => (
            <div
              key={registration.id}
              className="flex flex-col gap-2 rounded-sm border border-edge p-3 sm:flex-row sm:items-center sm:justify-between"
            >
              <div className="flex flex-col gap-1">
                <div className="flex items-center gap-2">
                  <span className="font-mono text-body text-foreground">
                    {providerById.get(registration.providerId)?.displayName ??
                      registration.providerId}
                  </span>
                  <RegistrationStatusBadge status={registration.status} />
                </div>
                {registration.rejectionReason != null && (
                  <FieldError>{registration.rejectionReason}</FieldError>
                )}
              </div>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => onReviewRegistration(registration.id)}
              >
                Review
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
