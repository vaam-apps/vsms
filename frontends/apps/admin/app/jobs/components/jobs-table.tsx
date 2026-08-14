// Dumb component (R6): markup, classes, and iteration over the rows it is
// handed. No data fetching, no tRPC, no business rules — `onRowClick`/
// `onRequeueClick` are callbacks the smart screen (`../jobs-screen.tsx`)
// owns; this file only wires them to the right DOM events. Moved verbatim
// out of `jobs-screen.tsx`, not rewritten — same classes, same structure.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import {
  Button,
  IdDisplay,
  InlineEmptyState,
  JobStatusPill,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
} from "@vsms/ui";
import { ChevronRight } from "lucide-react";

type RouterOutputs = inferRouterOutputs<AppRouter>;
export type JobListItem = RouterOutputs["jobs"]["list"]["items"][number];

// Column visibility: Attempts hides below `sm`, Run at below `md`, Last
// error/Id below `lg` — via `TableHead`/`TableCell`'s own `hideBelow` prop
// (`@vsms/ui`'s `primitives/table.tsx`), so head and cell share one
// breakpoint decision per column instead of two copies of the same class
// string that could silently drift apart. Mobile keeps
// State/Kind/Updated/Action (the 3–4 columns an operator needs to triage
// at a glance); everything else is one tap away in the quick-detail
// drawer.

export interface JobsTableProps {
  items: JobListItem[];
  isLoading: boolean;
  hasFilters: boolean;
  onClearFilters: () => void;
  onRowClick: (job: JobListItem) => void;
  onRequeueClick: (job: JobListItem) => void;
  requeuePending: boolean;
}

export function JobsTable({
  items,
  isLoading,
  hasFilters,
  onClearFilters,
  onRowClick,
  onRequeueClick,
  requeuePending,
}: JobsTableProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>State</TableHead>
          <TableHead>Kind</TableHead>
          <TableHead hideBelow="sm">Attempts</TableHead>
          <TableHead hideBelow="lg">Last error</TableHead>
          <TableHead hideBelow="md">Run at</TableHead>
          <TableHead hideBelow="lg">Id</TableHead>
          <TableHead align="end">Updated</TableHead>
          <TableHead align="end">Action</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {isLoading &&
          Array.from({ length: 8 }).map((_, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
            <TableRow key={i}>
              <TableCell colSpan={8}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          ))}

        {!isLoading && items.length === 0 && (
          <tr>
            <td colSpan={8}>
              <InlineEmptyState
                message={
                  hasFilters ? "No jobs match the current filters." : "No jobs in the backlog."
                }
                {...(hasFilters
                  ? { action: { label: "Clear filters", onClick: onClearFilters } }
                  : {})}
              />
            </td>
          </tr>
        )}

        {items.map((job) => (
          <TableRow
            key={job.id}
            tabIndex={0}
            role="button"
            aria-label={`View details for job ${job.kind}`}
            className="cursor-pointer"
            onClick={() => onRowClick(job)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onRowClick(job);
              }
            }}
          >
            <TableCell>
              <JobStatusPill state={job.state} />
            </TableCell>
            <TableCell mono className="max-w-[160px] truncate">
              {job.kind}
            </TableCell>
            <TableCell mono hideBelow="sm">
              {job.attempts}/{job.maxAttempts}
            </TableCell>
            <TableCell hideBelow="lg">
              {job.lastError != null ? (
                <span
                  className="line-clamp-1 max-w-[320px] text-caption text-muted-foreground"
                  title={job.lastError}
                >
                  {job.lastError}
                </span>
              ) : (
                <span className="text-muted-foreground">—</span>
              )}
            </TableCell>
            <TableCell hideBelow="md">
              <TimestampDisplay value={job.runAt} />
            </TableCell>
            <TableCell hideBelow="lg">
              <IdDisplay value={job.id} />
            </TableCell>
            <TableCell align="end">
              <TimestampDisplay value={job.updatedAt} />
            </TableCell>
            <TableCell align="end">
              <div className="flex items-center justify-end gap-1.5">
                {job.state === "dead" && (
                  <Button
                    type="button"
                    variant="secondary"
                    size="sm"
                    disabled={requeuePending}
                    onClick={(e) => {
                      e.stopPropagation();
                      onRequeueClick(job);
                    }}
                  >
                    Requeue
                  </Button>
                )}
                <ChevronRight
                  size={14}
                  strokeWidth={1.5}
                  aria-hidden="true"
                  className="text-subtle-foreground"
                />
              </div>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
