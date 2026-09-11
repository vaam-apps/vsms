import { DetailList, DetailRow, FieldError, TimestampDisplay } from "@vaam-apps/ui";
import { AttemptStatusPill } from "@/components/status";
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
    <DetailList>
      <DetailRow label="State">
        <AttemptStatusPill state={attempt.state} showLiteral />
      </DetailRow>
      <DetailRow label="Endpoint">
        <span className="max-w-[240px] truncate font-mono text-caption">{endpointUrl}</span>
      </DetailRow>
      <DetailRow label="Attempts">
        <span className="font-mono">{attempt.attempts}</span>
      </DetailRow>
      <DetailRow label="Last status code">
        <span className="font-mono">{attempt.lastStatusCode ?? "—"}</span>
      </DetailRow>
      {attempt.lastError != null && (
        <DetailRow label="Last error" variant="stacked">
          <FieldError>{attempt.lastError}</FieldError>
        </DetailRow>
      )}
      <DetailRow label="Last attempt">
        {attempt.lastAttemptAt != null ? (
          <TimestampDisplay value={attempt.lastAttemptAt} />
        ) : (
          <span className="text-muted-foreground">never</span>
        )}
      </DetailRow>
    </DetailList>
  );
}
