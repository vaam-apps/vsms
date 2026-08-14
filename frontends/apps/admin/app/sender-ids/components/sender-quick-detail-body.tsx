import { Button, InlineEmptyState } from "@vsms/ui";
import type { ProviderListItem, RegistrationListItem, SenderIdListItem } from "../sender-id-domain";
import { RegistrationStatusBadge } from "./registration-status-badge";

// Dumb (R6): the `QuickDetailDrawer`'s summary for one sender id — its own
// fields plus a per-registration Review shortcut.
export function SenderQuickDetailBody({
  senderId,
  registrations,
  providerById,
  onReviewRegistration,
}: {
  senderId: SenderIdListItem;
  registrations: RegistrationListItem[];
  providerById: Map<string, ProviderListItem>;
  onReviewRegistration: (registrationId: string) => void;
}) {
  return (
    <div className="flex flex-col gap-4">
      <dl className="flex flex-col gap-3 text-body">
        <div className="flex items-center justify-between gap-3">
          <dt className="text-muted-foreground">Active</dt>
          <dd>{senderId.active ? "yes" : "no"}</dd>
        </div>
        <div className="flex items-center justify-between gap-3">
          <dt className="text-muted-foreground">Kind</dt>
          <dd className="font-mono text-caption">{senderId.kind}</dd>
        </div>
        {senderId.notes != null && senderId.notes !== "" && (
          <div className="flex flex-col gap-1">
            <dt className="text-muted-foreground">Notes</dt>
            <dd className="text-caption">{senderId.notes}</dd>
          </div>
        )}
      </dl>

      <div className="flex flex-col gap-2 border-edge border-t pt-3">
        <p className="text-caption text-subtle-foreground">Registrations</p>
        {registrations.length === 0 ? (
          <InlineEmptyState message="Not registered with any provider yet." />
        ) : (
          registrations.map((registration) => (
            <div
              key={registration.id}
              className="flex items-center justify-between gap-2 rounded-sm border border-edge px-2 py-1.5"
            >
              <div className="flex min-w-0 items-center gap-2">
                <span className="truncate font-mono text-caption">
                  {providerById.get(registration.providerId)?.key ?? registration.providerId}
                </span>
                <RegistrationStatusBadge status={registration.status} />
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
          ))
        )}
      </div>
    </div>
  );
}
