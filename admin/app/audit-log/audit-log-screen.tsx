"use client";

// The Audit log screen (#58): a filtered, paged window over
// `cratestack_audit`, plus the anchor hash chain's own current status —
// "does this period's chain verify," not just a table dump. See
// `crates/sms-api/src/audit_log.rs`'s own module doc for the full
// mechanism this reads.
//
// # Console redesign (Phase 2, Admin group) — what changed and why
//
// This screen stays a **`Page`**, unchanged
// (docs/design/console-redesign.md §3: "a page-scale table, not a record
// of something else"), but a row can now be opened as a
// **`QuickDetailDrawer`** — a peek at one audited write's `before`/`after`
// values without leaving the filtered table underneath, matching the
// Mercury reference (§1.4) this rule is drawn from. Deliberately *not* a
// `MoreDetailDrawer`: there is no edit form to grow into (see this file's
// own "genuinely read-only" note below), and no route ownership either —
// D14 reserves that for drawers a caller needs to survive a refresh or
// share a link to, and re-opening the same row after a refresh is one
// click on the same table. `model`/`operation`/`actorId`/`since`/`until`
// **are** now `nuqs` URL state (`history: "push"`, matching
// `messages-screen.tsx`/`jobs-screen.tsx`'s own filter convention) —
// unlike the row drawer, a filtered *search* over the audit log is exactly
// the kind of thing worth bookmarking or sending to a teammate. `offset`
// stays URL state too, but `history: "replace"` — paging through one
// search isn't a new view worth a distinct back-button stop the way
// changing the search itself is.
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
// inferred, and the row drawer below is read-only for the identical
// reason — a peek at evidence, never an editor for it.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Button,
  InlineEmptyState,
  Input,
  Label,
  QuickDetailDrawer,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
} from "@vsms/ui";
import { parseAsInteger, parseAsString, parseAsStringEnum, useQueryStates } from "nuqs";
import { useEffect, useState } from "react";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type AuditLogEntry = RouterOutputs["auditLog"]["list"]["entries"][number];

const OPERATIONS = ["create", "update", "delete"] as const;
const LIMIT = 50;

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

/** Every entry's `primaryKey`/`actor`/`before`/`after` is JSON-encoded
 * text, not parsed further by `@vsms/gateway` (that module's own doc: "the
 * same convention `Provider.config`/`Route.config` already use for a
 * JSON-shaped `String` column"). Pretty-prints when it parses; falls back
 * to the raw string otherwise rather than hiding a value this screen can't
 * make sense of — an audit trail should never quietly drop something it
 * couldn't format. */
function prettyJson(raw: string | undefined): string | undefined {
  if (raw === undefined) return undefined;
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

function JsonBlock({ label, value }: { label: string; value: string | undefined }) {
  if (value === undefined) return null;
  return (
    <div className="flex flex-col gap-1.5">
      <p className="font-medium text-caption text-muted-foreground">{label}</p>
      <pre className="max-h-64 overflow-auto rounded-sm bg-base-100 p-3 font-mono text-[12px] text-foreground">
        {value}
      </pre>
    </div>
  );
}

// `entry`/`open` are separate, and this is always mounted (never
// conditionally rendered) for the identical reason `apps-screen.tsx`'s
// `AppDetailDrawer` doc gives — `vaul`'s close transition needs the
// drawer still in the DOM for at least one frame after `open` flips
// `false`. `entry` is nullable so this can render (closed) before any row
// has ever been clicked.
function AuditEntryDrawer({
  entry,
  open,
  onClose,
}: {
  entry: AuditLogEntry | null;
  open: boolean;
  onClose: () => void;
}) {
  return (
    <QuickDetailDrawer
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={entry !== null ? `${entry.model} · ${entry.operation}` : "Audit entry"}
      description={entry !== null && <TimestampDisplay value={entry.occurredAt} />}
    >
      {entry !== null && (
        <div className="flex flex-col gap-4">
          <dl className="flex flex-col gap-2 text-body">
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">Event id</dt>
              <dd className="truncate font-mono text-caption text-foreground">{entry.eventId}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">Request id</dt>
              <dd className="truncate font-mono text-caption text-foreground">
                {entry.requestId ?? <span className="text-subtle-foreground">none</span>}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">Tenant</dt>
              <dd className="truncate font-mono text-caption text-foreground">
                {entry.tenant ?? <span className="text-subtle-foreground">none</span>}
              </dd>
            </div>
          </dl>

          <JsonBlock label="Primary key" value={prettyJson(entry.primaryKey)} />
          <JsonBlock label="Actor" value={prettyJson(entry.actor)} />
          <JsonBlock label="Before" value={prettyJson(entry.before)} />
          <JsonBlock label="After" value={prettyJson(entry.after)} />
        </div>
      )}
    </QuickDetailDrawer>
  );
}

export function AuditLogScreen() {
  const [filters, setFilters] = useQueryStates(
    {
      model: parseAsString,
      operation: parseAsStringEnum<(typeof OPERATIONS)[number]>([...OPERATIONS]),
      actorId: parseAsString,
      since: parseAsString,
      until: parseAsString,
    },
    { history: "push" },
  );
  // Paging through one search isn't a new view worth its own back-button
  // stop the way changing the search is — see this file's own module doc.
  const [offset, setOffset] = useQueryStates(
    { offset: parseAsInteger.withDefault(0) },
    { history: "replace" },
  );
  const [selectedEntry, setSelectedEntry] = useState<AuditLogEntry | null>(null);
  // See `apps-screen.tsx`'s own `stickyPanelId` doc — the drawer stays
  // mounted below so its `vaul` close transition can play; this keeps its
  // content from blanking out mid-transition.
  const [stickyEntry, setStickyEntry] = useState<AuditLogEntry | null>(null);
  useEffect(() => {
    if (selectedEntry !== null) setStickyEntry(selectedEntry);
  }, [selectedEntry]);

  const listQuery = trpc.auditLog.list.useQuery({
    model: filters.model ?? undefined,
    operation: filters.operation ?? undefined,
    actorId: filters.actorId ?? undefined,
    since: filters.since ? `${filters.since}T00:00:00.000Z` : undefined,
    until: filters.until ? `${filters.until}T23:59:59.999Z` : undefined,
    limit: LIMIT,
    offset: offset.offset,
  });

  const hasFilters =
    (filters.model ?? "") !== "" ||
    filters.operation !== null ||
    (filters.actorId ?? "") !== "" ||
    (filters.since ?? "") !== "" ||
    (filters.until ?? "") !== "";

  function clearFilters() {
    void setFilters({ model: null, operation: null, actorId: null, since: null, until: null });
    void setOffset({ offset: 0 });
  }

  return (
    <div className="flex flex-col gap-6">
      <div className="border-edge border-b pb-6">
        <h1 className="font-medium text-foreground text-title">Audit log</h1>
        <p className="mt-1 max-w-xl text-body text-muted-foreground">
          Every audited write in this system, and whether the tamper-evidence chain over it still
          verifies. Read-only — see this screen&apos;s own note below.
        </p>
      </div>

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
            value={filters.model ?? ""}
            onChange={(e) => {
              void setFilters({ model: e.target.value === "" ? null : e.target.value });
              void setOffset({ offset: 0 });
            }}
            className="w-44"
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="audit-filter-operation">Operation</Label>
          <Select
            value={filters.operation ?? "any"}
            onValueChange={(value) => {
              void setFilters({
                operation: value === "any" ? null : (value as (typeof OPERATIONS)[number]),
              });
              void setOffset({ offset: 0 });
            }}
          >
            <SelectTrigger id="audit-filter-operation" className="w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="any">Any</SelectItem>
              {OPERATIONS.map((op) => (
                <SelectItem key={op} value={op}>
                  {op}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="audit-filter-actor">Actor id</Label>
          <Input
            id="audit-filter-actor"
            value={filters.actorId ?? ""}
            onChange={(e) => {
              void setFilters({ actorId: e.target.value === "" ? null : e.target.value });
              void setOffset({ offset: 0 });
            }}
            className="w-44"
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="audit-filter-since">Since</Label>
          <Input
            id="audit-filter-since"
            type="date"
            value={filters.since ?? ""}
            onChange={(e) => {
              void setFilters({ since: e.target.value === "" ? null : e.target.value });
              void setOffset({ offset: 0 });
            }}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="audit-filter-until">Until</Label>
          <Input
            id="audit-filter-until"
            type="date"
            value={filters.until ?? ""}
            onChange={(e) => {
              void setFilters({ until: e.target.value === "" ? null : e.target.value });
              void setOffset({ offset: 0 });
            }}
          />
        </div>
        {hasFilters && (
          <Button type="button" variant="ghost" size="sm" onClick={clearFilters}>
            Clear filters
          </Button>
        )}
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
            <TableHead className="hidden md:table-cell">Primary key</TableHead>
            <TableHead className="hidden sm:table-cell">Actor</TableHead>
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
            <TableRow>
              <TableCell colSpan={5}>
                <InlineEmptyState message="No matching audit entries." />
              </TableCell>
            </TableRow>
          )}
          {listQuery.data?.entries.map((entry: AuditLogEntry) => (
            <TableRow
              key={entry.eventId}
              className="cursor-pointer"
              onClick={() => setSelectedEntry(entry)}
            >
              <TableCell align="end">
                <TimestampDisplay value={entry.occurredAt} />
              </TableCell>
              <TableCell mono>{entry.model}</TableCell>
              <TableCell mono>{entry.operation}</TableCell>
              <TableCell className="hidden max-w-[220px] truncate font-mono text-caption md:table-cell">
                {entry.primaryKey}
              </TableCell>
              <TableCell className="hidden max-w-[260px] truncate font-mono text-caption sm:table-cell">
                {entry.actor}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <div className="flex items-center justify-between">
        <span className="text-caption text-subtle-foreground">
          Showing {listQuery.data?.entries.length ?? 0} entries starting at offset {offset.offset}
        </span>
        <div className="flex gap-2">
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={offset.offset === 0}
            onClick={() => void setOffset({ offset: Math.max(0, offset.offset - LIMIT) })}
          >
            Previous
          </Button>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={listQuery.data?.hasMore !== true}
            onClick={() => void setOffset({ offset: offset.offset + LIMIT })}
          >
            Next
          </Button>
        </div>
      </div>

      <AuditEntryDrawer
        entry={stickyEntry}
        open={selectedEntry !== null}
        onClose={() => setSelectedEntry(null)}
      />
    </div>
  );
}
