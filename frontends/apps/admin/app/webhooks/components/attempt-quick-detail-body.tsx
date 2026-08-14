import { AttemptStatusPill, TimestampDisplay } from "@vsms/ui";
import type { AttemptListItem } from "../webhook-domain";

// Dumb (R6): the `QuickDetailDrawer`'s summary `dl` for one delivery
// attempt.
export function AttemptQuickDetailBody({
  attempt,
  endpointUrl,
}: {
  attempt: AttemptListItem;
  endpointUrl: string;
}) {
  return (
    <dl className="flex flex-col gap-3 text-body">
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">State</dt>
        <dd>
          <AttemptStatusPill state={attempt.state} showLiteral />
        </dd>
      </div>
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Endpoint</dt>
        <dd className="max-w-[240px] truncate font-mono text-caption">{endpointUrl}</dd>
      </div>
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Attempts</dt>
        <dd className="font-mono">{attempt.attempts}</dd>
      </div>
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Last status code</dt>
        <dd className="font-mono">{attempt.lastStatusCode ?? "—"}</dd>
      </div>
      {attempt.lastError != null && (
        <div className="flex flex-col gap-1">
          <dt className="text-muted-foreground">Last error</dt>
          <dd className="text-caption text-state-danger-fg">{attempt.lastError}</dd>
        </div>
      )}
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Last attempt</dt>
        <dd>
          {attempt.lastAttemptAt != null ? (
            <TimestampDisplay value={attempt.lastAttemptAt} />
          ) : (
            <span className="text-muted-foreground">never</span>
          )}
        </dd>
      </div>
    </dl>
  );
}
