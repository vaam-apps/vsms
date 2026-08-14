import { Button } from "@vsms/ui";
import type { AttemptListItem } from "../webhook-domain";

// Dumb (R6): the attempt `QuickDetailDrawer`'s own footer (not shown while
// the inline replay confirmation is armed).
export function AttemptQuickDetailFooter({
  attempt,
  replayPending,
  onClose,
  onReplay,
  onViewPayload,
}: {
  attempt: AttemptListItem;
  replayPending: boolean;
  onClose: () => void;
  onReplay: () => void;
  onViewPayload: () => void;
}) {
  const canReplay = attempt.state === "failed" || attempt.state === "dead";
  return (
    <>
      <Button type="button" variant="ghost" size="sm" onClick={onClose}>
        Close
      </Button>
      {canReplay && (
        <Button
          type="button"
          variant="secondary"
          size="sm"
          disabled={replayPending}
          onClick={onReplay}
        >
          Replay
        </Button>
      )}
      <Button type="button" size="sm" onClick={onViewPayload}>
        View payload
      </Button>
    </>
  );
}
