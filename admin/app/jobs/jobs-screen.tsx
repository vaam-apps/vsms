"use client";

// The Jobs screen (#56): queue backlog, failed/dead jobs with their error,
// and re-enqueue. "Without this, a stuck job is invisible until something
// downstream breaks" — the issue's own words.
//
// # Why a plain poll, not a live stream
//
// `messages-screen.tsx` layers a live long-poll (`messages.onStateChange`)
// on top of its own list query because the design doc's §6.5 explicitly
// asks for that treatment on the flagship messages list. No such spec
// exists for jobs, and building a second server-side hub for a six-role,
// low-cardinality diagnostics table isn't proportionate to what this
// screen is for — an operator checking "is anything stuck," not watching a
// feed. `trpc.jobs.list.useQuery` with a `refetchInterval` is enough.
//
// # Why the visible whole-system-scope banner
//
// `Job` has no `appId` at all — unlike `Message`, there is no "this app's
// jobs" to scope to (`schema.cstack`'s own comment on `Job`'s `@@allow`).
// This screen shows the entire system's backlog. Same reasoning
// `messages-screen.tsx`'s own banner documents, opposite direction: that
// screen explains a narrower-than-expected scope, this one explains a
// broader one — both exist so an operator with no context for either
// doesn't mistake the shape for a bug.
//
// # Re-enqueue, precisely
//
// Only a `dead` job can be requeued — `requeueJob`'s own `409 Conflict` on
// anything else is the real guard (Postgres decides, this UI proposes);
// the button is simply never rendered for a row that isn't `dead`, so an
// operator is never invited to try an action the API would refuse anyway.
// Confirmed via a `Dialog`, not a bare click — the design doc's own rule
// (`toast.tsx`'s module doc) is that anything an operator must act on is
// inline, never a toast; a requeue's own confirmation is the row's status
// pill flipping from Dead to Pending on the very next poll, which is why
// this screen has no success toast at all — the state change *is* the
// confirmation.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  IdDisplay,
  InlineEmptyState,
  Input,
  JOB_STATES,
  type JobState,
  JobStatusPill,
  Label,
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
  ThemeToggle,
  TimestampDisplay,
} from "@vsms/ui";
import { parseAsString, parseAsStringEnum, useQueryStates } from "nuqs";
import { useState } from "react";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type JobListItem = RouterOutputs["jobs"]["list"]["items"][number];

const JOB_STATE_LABELS: Record<JobState, string> = {
  pending: "Pending",
  running: "Running",
  succeeded: "Succeeded",
  failed: "Failed (retrying)",
  dead: "Dead",
  cancelled: "Cancelled",
};

/** Diagnostics screens can afford a slower cadence than the messages
 * list's design-doc-mandated feel of "live" — the jobs role itself only
 * polls every `jobs::POLL_INTERVAL` (1s), and this is a human looking at a
 * table, not a feed. */
const REFETCH_INTERVAL_MS = 5000;

export function JobsScreen() {
  const [filters, setFilters] = useQueryStates(
    {
      state: parseAsStringEnum<JobState>([...JOB_STATES]),
      kind: parseAsString,
    },
    { history: "push" },
  );

  const listInput = {
    state: filters.state ?? undefined,
    kind: filters.kind && filters.kind !== "" ? filters.kind : undefined,
    limit: 200,
  };

  const listQuery = trpc.jobs.list.useQuery(listInput, {
    refetchInterval: REFETCH_INTERVAL_MS,
  });
  const utils = trpc.useUtils();
  const requeueMutation = trpc.jobs.requeue.useMutation({
    onSuccess: () => {
      void utils.jobs.list.invalidate();
    },
  });

  const [confirmTarget, setConfirmTarget] = useState<JobListItem | null>(null);

  function clearFilters() {
    void setFilters({ state: null, kind: null });
  }

  const hasFilters = filters.state !== null || (filters.kind ?? "") !== "";

  function confirmRequeue() {
    if (confirmTarget === null) return;
    requeueMutation.mutate({ jobId: confirmTarget.id });
    setConfirmTarget(null);
  }

  return (
    <main className="mx-auto flex max-w-[1400px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Jobs</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            The background job queue — backlog, retries, and dead jobs with their last error.
            Refreshes every {Math.round(REFETCH_INTERVAL_MS / 1000)}s.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <a
            href="/"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Composer
          </a>
          <a
            href="/messages"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Messages
          </a>
          <a
            href="/workers"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Workers
          </a>
          <ThemeToggle />
        </div>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Whole-system backlog, not scoped to one app —{" "}
        <span className="font-mono text-foreground">Job</span> has no app boundary to scope by. This
        is not a filter and not a bug.
      </div>

      {requeueMutation.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Requeue failed: {requeueMutation.error.message}
        </div>
      )}

      <div className="flex flex-wrap items-end gap-4">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="filter-state">State</Label>
          <Select
            value={filters.state ?? "__all"}
            onValueChange={(value) =>
              void setFilters({ state: value === "__all" ? null : (value as JobState) })
            }
          >
            <SelectTrigger id="filter-state" className="w-[200px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all">All states</SelectItem>
              {JOB_STATES.map((state) => (
                <SelectItem key={state} value={state}>
                  {JOB_STATE_LABELS[state]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="filter-kind">Kind</Label>
          <Input
            id="filter-kind"
            placeholder="e.g. expire_stale"
            className="w-[220px]"
            value={filters.kind ?? ""}
            onChange={(e) =>
              void setFilters({ kind: e.target.value === "" ? null : e.target.value })
            }
          />
        </div>

        {hasFilters && (
          <Button type="button" variant="ghost" size="sm" onClick={clearFilters}>
            Clear filters
          </Button>
        )}
      </div>

      {listQuery.data?.truncated && (
        <p className="text-caption text-subtle-foreground">
          Showing the most recently updated 1000 jobs — sms-api's `GET /jobs` has no server-side
          filter for state or kind, so filtering happens over that window. Older matches outside it
          won't appear.
        </p>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>State</TableHead>
            <TableHead>Kind</TableHead>
            <TableHead>Attempts</TableHead>
            <TableHead>Last error</TableHead>
            <TableHead>Run at</TableHead>
            <TableHead>Id</TableHead>
            <TableHead align="end">Updated</TableHead>
            <TableHead align="end">Action</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {listQuery.isLoading &&
            Array.from({ length: 8 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={8}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!listQuery.isLoading && (listQuery.data?.items.length ?? 0) === 0 && (
            <tr>
              <td colSpan={8}>
                <InlineEmptyState
                  message={
                    hasFilters ? "No jobs match the current filters." : "No jobs in the backlog."
                  }
                  {...(hasFilters
                    ? { action: { label: "Clear filters", onClick: clearFilters } }
                    : {})}
                />
              </td>
            </tr>
          )}

          {listQuery.data?.items.map((job) => (
            <TableRow key={job.id}>
              <TableCell>
                <JobStatusPill state={job.state} />
              </TableCell>
              <TableCell mono>{job.kind}</TableCell>
              <TableCell mono>
                {job.attempts}/{job.maxAttempts}
              </TableCell>
              <TableCell>
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
              <TableCell>
                <TimestampDisplay value={job.runAt} />
              </TableCell>
              <TableCell>
                <IdDisplay value={job.id} />
              </TableCell>
              <TableCell align="end">
                <TimestampDisplay value={job.updatedAt} />
              </TableCell>
              <TableCell align="end">
                {job.state === "dead" && (
                  <Button
                    type="button"
                    variant="secondary"
                    size="sm"
                    disabled={requeueMutation.isPending}
                    onClick={() => setConfirmTarget(job)}
                  >
                    Requeue
                  </Button>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <Dialog
        open={confirmTarget !== null}
        onOpenChange={(open) => !open && setConfirmTarget(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Requeue this job?</DialogTitle>
            <DialogDescription>
              {confirmTarget != null && (
                <>
                  <span className="font-mono text-foreground">{confirmTarget.kind}</span> failed{" "}
                  {confirmTarget.attempts} time{confirmTarget.attempts === 1 ? "" : "s"} and is now
                  dead. Requeuing resets its attempts counter to 0 and moves it back to pending,
                  where the next <span className="font-mono">jobs</span> poll will pick it up again.
                </>
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setConfirmTarget(null)}>
              Cancel
            </Button>
            <Button type="button" onClick={confirmRequeue}>
              Requeue
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </main>
  );
}
