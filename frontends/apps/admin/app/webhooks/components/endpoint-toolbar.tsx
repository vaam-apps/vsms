import { Button, InlineBanner } from "@vsms/ui";

// Dumb (R6): the role-scope notice, the "Endpoints" section heading with
// its "New endpoint" action, and a list read-error banner.
export function EndpointToolbar({
  listErrorMessage,
  onNewEndpoint,
}: {
  listErrorMessage?: string | undefined;
  onNewEndpoint: () => void;
}) {
  return (
    <>
      <InlineBanner variant="neutral">
        Reads and writes act as you — endpoint saves and secret rotation require{" "}
        <span className="font-mono text-foreground">webhook:manage</span> (owner, admin, and
        developer all carry it by default). The secret shown below is the live value, not a
        placeholder — masked here as a screen-share precaution, not a security boundary; see the
        screen's own note for why.
      </InlineBanner>

      <div className="flex items-center justify-between">
        <h2 className="font-medium text-body text-foreground">Endpoints</h2>
        <Button type="button" size="sm" onClick={onNewEndpoint}>
          New endpoint
        </Button>
      </div>

      {listErrorMessage != null && (
        <InlineBanner variant="danger">
          Could not read webhook endpoints: {listErrorMessage}
        </InlineBanner>
      )}
    </>
  );
}
