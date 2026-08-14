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

/** Column visibility shared between `TableHead` and `TableCell` so a
 * breakpoint hides both halves of a column together — misaligning them
 * would shift every cell after it. Mobile keeps State/Kind/Updated/Action
 * (the 3–4 columns an operator needs to triage at a glance); everything
 * else is one tap away in the quick-detail drawer. */
const COL_ATTEMPTS = "hidden sm:table-cell";
const COL_RUN_AT = "hidden md:table-cell";
const COL_LAST_ERROR = "hidden lg:table-cell";
const COL_ID = "hidden lg:table-cell";

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
          <TableHead className={COL_ATTEMPTS}>Attempts</TableHead>
          <TableHead className={COL_LAST_ERROR}>Last error</TableHead>
          <TableHead className={COL_RUN_AT}>Run at</TableHead>
          <TableHead className={COL_ID}>Id</TableHead>
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
            <TableCell mono className={COL_ATTEMPTS}>
              {job.attempts}/{job.maxAttempts}
            </TableCell>
            <TableCell className={COL_LAST_ERROR}>
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
            <TableCell className={COL_RUN_AT}>
              <TimestampDisplay value={job.runAt} />
            </TableCell>
            <TableCell className={COL_ID}>
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
