import { InlineConfirm } from "@vsms/ui";
import type { EndpointListItem } from "../webhook-domain";

// Dumb (R6): the delete confirmation, rendered *inline* inside
// `MoreDetailDrawer`'s own body — never a nested `Dialog`. See
// `frontends/apps/admin/app/gallery/page.tsx`'s
// `NestedDialogInDrawerRegression` for why.
export function EndpointDeleteConfirm({
  endpoint,
  pending,
  errorMessage,
  onConfirm,
  onCancel,
}: {
  endpoint: EndpointListItem;
  pending: boolean;
  errorMessage?: string | undefined;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <InlineConfirm
      title="Delete this endpoint?"
      description={
        <>
          Stops all future deliveries to{" "}
          <span className="font-mono text-foreground">{endpoint.url}</span>. Attempts already
          recorded against it are not deleted.
        </>
      }
      confirmLabel="Delete"
      pendingLabel="Deleting…"
      pending={pending}
      error={errorMessage != null ? `Delete failed: ${errorMessage}` : undefined}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}
