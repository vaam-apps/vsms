import { Button } from "@vsms/ui";

// Dumb, route-local (R6): the more-detail drawer's own footer for the
// create/edit form (not shown while the inline delete confirmation is
// armed — the screen swaps `footer` to `undefined` for that case, since
// `InlineConfirm` supplies its own action row).
export function RouteEditFooter({
  showDelete,
  isCreate,
  pending,
  onDelete,
  onCancel,
}: {
  showDelete: boolean;
  isCreate: boolean;
  pending: boolean;
  onDelete: () => void;
  onCancel: () => void;
}) {
  return (
    <>
      {showDelete && (
        <Button
          type="button"
          variant="destructive"
          size="sm"
          className="mr-auto"
          onClick={onDelete}
        >
          Delete
        </Button>
      )}
      <Button type="button" variant="ghost" onClick={onCancel}>
        Cancel
      </Button>
      <Button type="submit" form="route-edit-form" disabled={pending}>
        {pending ? "Saving…" : isCreate ? "Create" : "Save"}
      </Button>
    </>
  );
}
