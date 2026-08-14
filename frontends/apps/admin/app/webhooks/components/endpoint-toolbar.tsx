import { Button } from "@vsms/ui";

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
      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Reads and writes act as you — endpoint saves and secret rotation require{" "}
        <span className="font-mono text-foreground">webhook:manage</span> (owner, admin, and
        developer all carry it by default). The secret shown below is the live value, not a
        placeholder — masked here as a screen-share precaution, not a security boundary; see the
        screen's own note for why.
      </div>

      <div className="flex items-center justify-between">
        <h2 className="font-medium text-body text-foreground">Endpoints</h2>
        <Button type="button" size="sm" onClick={onNewEndpoint}>
          New endpoint
        </Button>
      </div>

      {listErrorMessage != null && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read webhook endpoints: {listErrorMessage}
        </div>
      )}
    </>
  );
}
