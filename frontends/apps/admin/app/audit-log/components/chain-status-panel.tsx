// Dumb view for the audit chain status banner (R6). The smart layer
// (`audit-log-screen.tsx`) owns the `auditLog.chainStatus` query and
// collapses it into one of the five states below; this component only
// renders whichever one it is handed.

import { InlineBanner, Skeleton, TimestampDisplay } from "@vaam-apps/ui";

export type ChainStatusPanelProps =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | { kind: "no-anchor" }
  | { kind: "ok"; rowCount: number; periodEnd: string | undefined }
  | { kind: "broken"; linkageBreakCount: number; contentVerified: boolean | undefined };

export function ChainStatusPanel(props: ChainStatusPanelProps) {
  if (props.kind === "loading") {
    return <Skeleton className="h-10 w-full" />;
  }

  if (props.kind === "error") {
    return (
      <InlineBanner variant="danger">
        Could not read the audit chain status: {props.message}
      </InlineBanner>
    );
  }

  if (props.kind === "no-anchor") {
    return (
      <InlineBanner variant="neutral">
        No audit anchor has been written yet. The <span className="font-mono">anchor_audit</span>{" "}
        job writes one hourly once a worker runs the <span className="font-mono">jobs</span> role.
      </InlineBanner>
    );
  }

  const broken = props.kind === "broken";

  return (
    <InlineBanner variant={broken ? "danger" : "success"}>
      {props.kind === "broken" ? (
        <>
          Chain verification found a problem — possible tampering.{" "}
          {props.linkageBreakCount > 0 && <>{props.linkageBreakCount} linkage break(s). </>}
          {props.contentVerified === false && (
            <>The latest anchor's own content no longer matches.</>
          )}
        </>
      ) : (
        <>
          The audit chain verifies. The latest anchor covers{" "}
          {props.kind === "ok" ? props.rowCount : 0} rows through{" "}
          {props.kind === "ok" && props.periodEnd !== undefined && (
            <TimestampDisplay value={props.periodEnd} />
          )}
          .
        </>
      )}
      <span className="mt-1 block text-body-small text-muted-foreground">
        One limit: deleting the newest anchor, before a later one references it, cannot be detected.
      </span>
    </InlineBanner>
  );
}
