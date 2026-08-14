import { Button } from "@vsms/ui";

// Dumb (R6): the sender id `QuickDetailDrawer`'s own footer.
export function SenderQuickDetailFooter({
  onClose,
  onViewFullDetails,
}: {
  onClose: () => void;
  onViewFullDetails: () => void;
}) {
  return (
    <>
      <Button type="button" variant="ghost" size="sm" onClick={onClose}>
        Close
      </Button>
      <Button type="button" size="sm" onClick={onViewFullDetails}>
        View full details
      </Button>
    </>
  );
}
