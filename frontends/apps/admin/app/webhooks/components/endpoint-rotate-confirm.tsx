import { InlineConfirm } from "@vsms/ui";

// Dumb (R6): the secret-rotation confirmation, rendered *inline* inside
// `MoreDetailDrawer`'s own body — never a nested `Dialog`. See
// `frontends/apps/admin/app/gallery/page.tsx`'s
// `NestedDialogInDrawerRegression` for why.
export function EndpointRotateConfirm({
  pending,
  onConfirm,
  onCancel,
}: {
  pending: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <InlineConfirm
      title="Rotate this endpoint's secret?"
      description={
        <>
          A new secret is minted immediately and every future delivery is signed with it. The{" "}
          <strong>current</strong> secret keeps verifying as the "previous secret" — not for a fixed
          time, but until you rotate <em>again</em>. If your receiver hasn't been updated to accept
          the new value before that happens, its signature checks will start failing at that point,
          not now.
        </>
      }
      confirmLabel="Rotate secret"
      pendingLabel="Rotating…"
      pending={pending}
      // Not danger-hued: the original centered-`Dialog` version of this
      // confirm used the default (primary) button, not the destructive
      // one — rotation has a real consequence (see the copy above) but
      // isn't itself the kind of irreversible-data-loss action the danger
      // hue is reserved for.
      destructive={false}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}
