// Dumb view for the audit chain status banner (R6). The smart layer
// (`audit-log-screen.tsx`) owns the `auditLog.chainStatus` query and
// collapses it into one of the five states below; this component only
// renders whichever one it is handed.

import { Skeleton, TimestampDisplay } from "@vsms/ui";

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
      <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
        Could not read the audit chain status: {props.message}
      </div>
    );
  }

  if (props.kind === "no-anchor") {
    return (
      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        No audit anchor has been written yet — the <span className="font-mono">anchor_audit</span>{" "}
        job (§7.5) runs hourly once a worker with the <span className="font-mono">jobs</span> role
        is up.
      </div>
    );
  }

  const broken = props.kind === "broken";

  return (
    <div
      className={
        broken
          ? "rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg"
          : "rounded-sm border border-state-success-border bg-state-success-bg px-3 py-2 text-caption text-state-success-fg"
      }
    >
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
          The audit chain verifies. Latest anchor covers {props.kind === "ok" ? props.rowCount : 0}{" "}
          row(s) through{" "}
          {props.kind === "ok" && props.periodEnd !== undefined && (
            <TimestampDisplay value={props.periodEnd} />
          )}
          .
        </>
      )}
      <span className="ml-2 text-micro text-subtle-foreground">
        (Cannot detect deletion of the single newest anchor before anything else references it — see
        OPEN_QUESTIONS.md §3.3.)
      </span>
    </div>
  );
}
