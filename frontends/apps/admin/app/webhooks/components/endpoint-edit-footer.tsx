import { Button } from "@vsms/ui";

// Dumb (R6): the more-detail drawer's own footer for the endpoint edit
// form (not shown while an inline confirmation — delete or rotate — is
// armed; the screen swaps `footer` to `undefined` for those, since
// `InlineConfirm` supplies its own action row).
export function EndpointEditFooter({
  pending,
  onDelete,
  onClose,
}: {
  pending: boolean;
  onDelete: () => void;
  onClose: () => void;
}) {
  return (
    <>
      <Button type="button" variant="destructive" size="sm" className="mr-auto" onClick={onDelete}>
        Delete
      </Button>
      <Button type="button" variant="ghost" onClick={onClose}>
        Close
      </Button>
      <Button type="submit" form="endpoint-edit-form" disabled={pending}>
        {pending ? "Saving…" : "Save"}
      </Button>
    </>
  );
}
