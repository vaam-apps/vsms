import { Button } from "@vsms/ui";

// Dumb (R6): the registration-review drawer's own footer (not shown while
// the inline resubmit confirmation is armed).
export function RegistrationReviewFooter({
  showResubmit,
  pending,
  onResubmit,
  onClose,
}: {
  showResubmit: boolean;
  pending: boolean;
  onResubmit: () => void;
  onClose: () => void;
}) {
  return (
    <>
      {showResubmit && (
        <Button
          type="button"
          variant="secondary"
          size="sm"
          className="mr-auto"
          onClick={onResubmit}
        >
          Resubmit
        </Button>
      )}
      <Button type="button" variant="ghost" onClick={onClose}>
        Close
      </Button>
      <Button type="submit" form="registration-review-form" disabled={pending}>
        {pending ? "Saving…" : "Save"}
      </Button>
    </>
  );
}
