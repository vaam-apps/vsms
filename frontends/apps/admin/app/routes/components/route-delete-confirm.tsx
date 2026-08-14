import { InlineConfirm } from "@vsms/ui";
import type { RouteListItem } from "../route-domain";

// Dumb, route-local (R6): the delete confirmation, rendered *inline* inside
// `MoreDetailDrawer`'s own body — never a nested `Dialog`. See
// `frontends/apps/admin/app/gallery/page.tsx`'s
// `NestedDialogInDrawerRegression` for why: a centered Headless UI `Dialog`
// nested inside an open `vaul` drawer never becomes visible, because
// `vaul`'s own `FocusScope` keeps forcing focus back out of the `Dialog`'s
// separate portal.
export function RouteDeleteConfirm({
  route,
  pending,
  errorMessage,
  onConfirm,
  onCancel,
}: {
  route: RouteListItem;
  pending: boolean;
  errorMessage?: string | undefined;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <InlineConfirm
      title="Delete this route?"
      description={
        <>
          <span className="font-mono text-foreground">{route.name}</span> will be removed
          permanently. This cannot be undone.
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
