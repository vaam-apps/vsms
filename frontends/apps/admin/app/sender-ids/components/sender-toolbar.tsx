import { Button, InlineBanner } from "@vsms/ui";

// Dumb (R6): the role-scope notice, list read-error banner, and the "All
// sender IDs" section heading with its "New sender ID" action.
export function SenderToolbar({
  errorMessage,
  onNewSenderId,
}: {
  errorMessage?: string | undefined;
  onNewSenderId: () => void;
}) {
  return (
    <>
      <InlineBanner variant="neutral">
        Reads and writes act as you — saving here requires your own role to carry{" "}
        <span className="font-mono text-foreground">sender:manage</span> (owner, admin, and operator
        all do by default).
      </InlineBanner>

      {errorMessage != null && (
        <InlineBanner variant="danger">Could not read sender IDs: {errorMessage}</InlineBanner>
      )}

      <div className="flex items-center justify-between">
        <h2 className="font-medium text-body text-foreground">All sender IDs</h2>
        <Button type="button" size="sm" onClick={onNewSenderId}>
          New sender ID
        </Button>
      </div>
    </>
  );
}
