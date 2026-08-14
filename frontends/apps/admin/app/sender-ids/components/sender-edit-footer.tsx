import { Button } from "@vsms/ui";

// Dumb (R6): the sender id more-detail drawer's own footer (not shown
// while the inline "register with a provider" form is armed — the screen
// swaps `footer` to `undefined` for that, since `InlineConfirm` supplies
// its own action row).
export function SenderEditFooter({ pending, onClose }: { pending: boolean; onClose: () => void }) {
  return (
    <>
      <Button type="button" variant="ghost" onClick={onClose}>
        Close
      </Button>
      <Button type="submit" form="sender-id-edit-form" disabled={pending}>
        {pending ? "Saving…" : "Save"}
      </Button>
    </>
  );
}
