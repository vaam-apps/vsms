"use client";

// The Audit log screen (#58): a filtered, paged window over
// `cratestack_audit`, plus the anchor hash chain's own current status —
// "does this period's chain verify," not just a table dump. See
// `crates/sms-api/src/audit_log.rs`'s own module doc for the full
// mechanism this reads.
//
// # Genuinely read-only — no write path exists anywhere on this screen
//
// There is no edit/delete action anywhere below because there is nothing
// to call: `crates/sms-api/src/procedures.rs` never constructs an
// `AuditAnchor` update or delete, and — checked live against a real
// Postgres, not assumed — no role, human or synthetic, can write one
// through any path this codebase exposes at all (`audit_log.rs`'s own
// module doc has the exact runtime error captured). This screen's own copy
// says so rather than leaving "why is there no edit button" to be
// inferred.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Button,
  InlineEmptyState,
  Input,
  Label,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
} from "@vsms/ui";
import { useState } from "react";
import { ConsoleNav } from "../console-nav";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type AuditLogEntry = RouterOutputs["auditLog"]["list"]["entries"][number];

function ChainStatusBanner() {
  const statusQuery = trpc.auditLog.chainStatus.useQuery();

  if (statusQuery.isLoading) {
    return <Skeleton className="h-10 w-full" />;
  }
  if (statusQuery.isError) {
    return (
      <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
        Could not read the audit chain status: {statusQuery.error.message}
      </div>
    );
  }
  const status = statusQuery.data;
  if (status === undefined) return null;

  if (status.latestAnchorId === undefined) {
    return (
      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        No audit anchor has been written yet — the <span className="font-mono">anchor_audit</span>{" "}
        job (§7.5) runs hourly once a worker with the <span className="font-mono">jobs</span> role
        is up.
      </div>
    );
  }

  const broken = status.linkageBreaks.length > 0 || status.latestContentVerified === false;

  return (
    <div
      className={
        broken
          ? "rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg"
          : "rounded-sm border border-state-success-border bg-state-success-bg px-3 py-2 text-caption text-state-success-fg"
      }
    >
      {broken ? (
        <>
          Chain verification found a problem — possible tampering.{" "}
          {status.linkageBreaks.length > 0 && <>{status.linkageBreaks.length} linkage break(s). </>}
          {status.latestContentVerified === false && (
            <>The latest anchor's own content no longer matches.</>
          )}
        </>
      ) : (
        <>
          The audit chain verifies. Latest anchor covers {status.latestRowCount ?? 0} row(s) through{" "}
          {status.latestPeriodEnd !== undefined && (
            <TimestampDisplay value={status.latestPeriodEnd} />
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

export function AuditLogScreen() {
  const [model, setModel] = useState("");
  const [actorId, setActorId] = useState("");
  const [offset, setOffset] = useState(0);
  const limit = 50;

  const listQuery = trpc.auditLog.list.useQuery({
    model: model.trim().length > 0 ? model.trim() : undefined,
    actorId: actorId.trim().length > 0 ? actorId.trim() : undefined,
    limit,
    offset,
  });

  return (
    <main className="mx-auto flex max-w-[1200px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Audit log</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Every audited write in this system, and whether the tamper-evidence chain over it still
            verifies. Read-only — see this screen&apos;s own note below.
          </p>
        </div>
        <ConsoleNav current="/audit-log" />
      </header>

      <ChainStatusBanner />

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        This view is genuinely read-only, not just missing an edit button — no role, including{" "}
        <span className="font-mono text-foreground">system</span>, can write an audit anchor through
        any path this codebase exposes.
      </div>

      <div className="flex flex-wrap items-end gap-3">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="audit-filter-model">Model</Label>
          <Input
            id="audit-filter-model"
            placeholder="App, User, Provider…"
            value={model}
            onChange={(e) => {
              setModel(e.target.value);
              setOffset(0);
            }}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="audit-filter-actor">Actor id</Label>
          <Input
            id="audit-filter-actor"
            value={actorId}
            onChange={(e) => {
              setActorId(e.target.value);
              setOffset(0);
            }}
          />
        </div>
      </div>

      {listQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read the audit log: {listQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead align="end">When</TableHead>
            <TableHead>Model</TableHead>
            <TableHead>Operation</TableHead>
            <TableHead>Primary key</TableHead>
            <TableHead>Actor</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {listQuery.isLoading && (
            <TableRow>
              <TableCell colSpan={5}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          )}
          {!listQuery.isLoading && (listQuery.data?.entries.length ?? 0) === 0 && (
            <tr>
              <td colSpan={5}>
                <InlineEmptyState message="No matching audit entries." />
              </td>
            </tr>
          )}
          {listQuery.data?.entries.map((entry: AuditLogEntry) => (
            <TableRow key={entry.eventId}>
              <TableCell align="end">
                <TimestampDisplay value={entry.occurredAt} />
              </TableCell>
              <TableCell mono>{entry.model}</TableCell>
              <TableCell mono>{entry.operation}</TableCell>
              <TableCell className="max-w-[220px] truncate font-mono text-caption">
                {entry.primaryKey}
              </TableCell>
              <TableCell className="max-w-[260px] truncate font-mono text-caption">
                {entry.actor}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <div className="flex items-center justify-between">
        <span className="text-caption text-subtle-foreground">
          Showing {listQuery.data?.entries.length ?? 0} entries starting at offset {offset}
        </span>
        <div className="flex gap-2">
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={offset === 0}
            onClick={() => setOffset(Math.max(0, offset - limit))}
          >
            Previous
          </Button>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={listQuery.data?.hasMore !== true}
            onClick={() => setOffset(offset + limit)}
          >
            Next
          </Button>
        </div>
      </div>
    </main>
  );
}
