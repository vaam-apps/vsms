import { InlineConfirm } from "@vsms/ui";
import type { ProviderListItem, RegistrationListItem } from "../sender-id-domain";

// Dumb (R6): the resubmit confirmation, rendered *inline* inside the
// registration-review `MoreDetailDrawer` body — never a nested `Dialog`.
// This is the "stacked" case: the registration-review drawer is itself a
// second `MoreDetailDrawer` opened from inside the sender id's own
// more-detail drawer, and the same broken-focus-trap mechanism applies
// regardless of nesting depth. See
// `frontends/apps/admin/app/gallery/page.tsx`'s
// `NestedDialogInDrawerRegression` for the full root-cause writeup.
export function ResubmitConfirm({
  registration,
  providerById,
  pending,
  onConfirm,
  onCancel,
}: {
  registration: RegistrationListItem;
  providerById: Map<string, ProviderListItem>;
  pending: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const providerName =
    providerById.get(registration.providerId)?.displayName ?? registration.providerId;
  return (
    <InlineConfirm
      title="Resubmit this registration?"
      description={
        <>
          Moves <span className="font-mono text-foreground">{providerName}</span> back to{" "}
          <span className="font-mono">pending</span>, stamps a fresh submitted-at, and clears the
          rejection reason — use this once whatever the provider objected to is actually fixed, not
          before.
        </>
      }
      confirmLabel="Resubmit"
      pendingLabel="Resubmitting…"
      destructive={false}
      pending={pending}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}
