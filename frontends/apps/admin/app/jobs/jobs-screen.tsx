"use client";

// The Jobs screen (#56): queue backlog, failed/dead jobs with their error,
// and re-enqueue. "Without this, a stuck job is invisible until something
// downstream breaks" — the issue's own words.
//
// Console redesign (docs/design/console-redesign.md, Phase 2 "Operations"):
// dropped the old per-screen hand-rolled `<header>` nav block in favor of
// `ConsoleShell`'s own sidebar/top bar (§7 build order: "no screen should
// still hand-roll a <header> nav after Phase 2") and added a row-click
// `QuickDetailDrawer` (§3/D14) so the full record — `dedupeKey`,
// `leaseOwner`/`leaseUntil`, `startedAt`/`finishedAt`, the untruncated
// `lastError`, the full id — is one tap away without needing every column
// visible on a phone. No "View full details" escalation to a wide/"more"
// drawer (§3's own generic Jobs bullet mentions one): `Job` has no edit
// form and no nested history, so the narrow drawer already holds the
// entire record — there is nothing a wider drawer would add.
//
// R6 (AGENTS.md): this file is the smart component — data fetching, URL
// state, mutations, handlers, derived values, and a tree of dumb
// components. No `className`, no raw markup. Markup and classes moved
// verbatim into `components/jobs-table.tsx`, `components/job-filters-
// bar.tsx`, `components/job-detail-fields.tsx` and `components/requeue-
// confirm-dialog.tsx`; the four hoisted `COL_*` class consts moved with
// the table, and the hand-maintained `JOB_STATE_LABELS` map is gone —
// it duplicated `JOB_STATUS_META[state].label` verbatim (checked entry by
// entry), so `job-filters-bar.tsx` reads the label straight off the same
// status table `JobStatusPill` already uses, the same derivation
// `messages-screen.tsx`'s own `STATE_LABELS` already uses for messages.
// `REFETCH_INTERVAL_MS` is gone too — `pollMs` now arrives as a prop, read
// server-side from `@vsms/env`'s `DIAGNOSTICS_POLL_MS` (`page.tsx`), the
// same `MESSAGE_STREAM_POLL_MS` pattern `messages-screen.tsx` already
// established, so a client component never needs a `NEXT_PUBLIC_*` copy of
// a server-validated tuning value.
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
// # Why the description says "every app"
//
// `Job` has no `appId` at all — unlike `Message`, there is no "this app's
// jobs" to scope to (`schema.cstack`'s own comment on `Job`'s `@@allow`).
// This screen shows the entire system's backlog, and the header says so in
// one clause rather than a banner, so an operator with no context does not
// mistake the breadth for a filter.
//
// # Paging
//
// The screen used to ask for 200 rows and render them all, a page
// thousands of pixels tall. `jobs.list` already takes `limit`/`offset` and
// answers `totalCount` (the gateway filters its own 1000-row window), so the
// screen asks for one page of `LIST_PAGE_SIZE` rows (`@vsms/env`) and keeps
// the page number in the URL beside the filters. A filter change resets it
// to the first page.
//
// # Re-enqueue, precisely
//
// Only a `dead` job can be requeued — `requeueJob`'s own `409 Conflict` on
// anything else is the real guard (Postgres decides, this UI proposes);
// the button is simply never rendered for a row that isn't `dead`, so an
// operator is never invited to try an action the API would refuse anyway.
// Confirmed via a dialog, not a bare click — the design doc's own rule
// (`toast.tsx`'s module doc) is that anything an operator must act on is
// inline, never a toast; a requeue's own confirmation is the row's status
// pill flipping from Dead to Pending on the very next poll, which is why
// this screen has no success toast at all — the state change *is* the
// confirmation. The confirm dialog can be reached from the table row's own
// action *or* from inside the quick-detail drawer (§3's own footnote: "a
// form can open a confirmation without contradicting the rule above" — a
// drawer opening a nested `Dialog`), both driving the same `confirm` URL
// param — see `components/requeue-confirm-dialog.tsx`'s own doc for a real
// limitation of that combination this rewrite did not introduce.
//
// # `detail`/`confirm` are ids in the URL, not copies of a server row
//
// The pre-R6 version of this screen held `detailJob`/`confirmTarget` as
// `useState<JobListItem | null>` — full copies of a fetched row. That's a
// second source of truth: `detailJob` needed its own `liveDetailJob = ...
// ?? detailJob` staleness fallback to paper over the copy going stale
// between polls, and `confirmTarget` had the identical problem. Both are
// gone. `detail`/`confirm` hold only the job **id**, in the URL (`nuqs`,
// alongside the existing `state`/`kind` filters — one `useQueryStates`
// call for the screen's whole UI position, per R6's "Grouped URL state").
// The row itself is always looked up fresh out of `listQuery.data` by that
// id; when the id doesn't match anything in the current page (filtered
// out, or the 1000-row window moved past it), the derived value is simply
// `null` and the drawer/dialog close themselves — no fallback needed
// because there is no copy left to go stale.
//
// `detail`/`confirm` use `history: "replace"`, not the filters' `"push"`:
// opening a drawer or a confirm dialog while browsing the backlog
// shouldn't grow a back-button trail one entry per row inspected.

import { Button, InlineBanner, Pagination, QuickDetailDrawer, ScreenStack } from "@vaam-apps/ui";
import { trpc } from "@vsms/hooks";
import { parseAsInteger, parseAsString, parseAsStringEnum, useQueryStates } from "nuqs";
import { ListSurface } from "@/components/list-surface";
import { LiveChip } from "@/components/live-chip";
import { PageHeader } from "@/components/page-header";
import { JOB_STATES, type JobState } from "@/components/status";
import { JobDetailFields } from "./components/job-detail-fields";
import { JobFiltersBar } from "./components/job-filters-bar";
import { type JobListItem, JobsTable } from "./components/jobs-table";
import { RequeueConfirmDialog } from "./components/requeue-confirm-dialog";

export interface JobsScreenProps {
  /** `DIAGNOSTICS_POLL_MS`, read server-side (`page.tsx`) — see this file's
   * own module doc. */
  pollMs: number;
  /** `LIST_PAGE_SIZE`, read server-side (`page.tsx`). */
  pageSize: number;
}

export function JobsScreen({ pollMs, pageSize }: JobsScreenProps) {
  const [urlState, setUrlState] = useQueryStates(
    {
      state: parseAsStringEnum<JobState>([...JOB_STATES]),
      kind: parseAsString,
      page: parseAsInteger.withDefault(1),
      detail: parseAsString.withOptions({ history: "replace" }),
      confirm: parseAsString.withOptions({ history: "replace" }),
    },
    { history: "push" },
  );

  const listInput = {
    state: urlState.state ?? undefined,
    kind: urlState.kind && urlState.kind !== "" ? urlState.kind : undefined,
    limit: pageSize,
    offset: (Math.max(1, urlState.page) - 1) * pageSize,
  };

  const listQuery = trpc.jobs.list.useQuery(listInput, {
    refetchInterval: pollMs,
  });
  const utils = trpc.useUtils();
  const requeueMutation = trpc.jobs.requeue.useMutation({
    onSuccess: () => {
      void utils.jobs.list.invalidate();
    },
  });

  function clearFilters() {
    void setUrlState({ state: null, kind: null, page: null });
  }

  const hasFilters = urlState.state !== null || (urlState.kind ?? "") !== "";

  const items: JobListItem[] = listQuery.data?.items ?? [];
  const page = Math.max(1, urlState.page);
  const total = listQuery.data?.totalCount ?? 0;
  const pageCount = Math.max(1, Math.ceil(total / pageSize));
  const detailJob =
    urlState.detail === null ? null : (items.find((j) => j.id === urlState.detail) ?? null);
  const confirmJob =
    urlState.confirm === null ? null : (items.find((j) => j.id === urlState.confirm) ?? null);

  function openDetail(job: JobListItem) {
    void setUrlState({ detail: job.id });
  }

  function closeDetail() {
    void setUrlState({ detail: null });
  }

  function openConfirm(job: JobListItem) {
    void setUrlState({ confirm: job.id });
  }

  function closeConfirm() {
    void setUrlState({ confirm: null });
  }

  function confirmRequeue() {
    if (confirmJob === null) return;
    requeueMutation.mutate({ jobId: confirmJob.id });
    closeConfirm();
  }

  const truncatedNotice = listQuery.data?.truncated ?? false;

  return (
    <ScreenStack>
      <PageHeader
        title="Jobs"
        description="The background queue for every app: backlog, retries and dead jobs with their last error."
        actions={
          <LiveChip
            live={!listQuery.isError}
            label={`Updating every ${Math.round(pollMs / 1000)} s`}
          />
        }
      />

      {requeueMutation.isError && (
        <InlineBanner variant="danger">
          Requeue failed: {requeueMutation.error.message}
        </InlineBanner>
      )}

      <ListSurface
        toolbar={
          <JobFiltersBar
            state={urlState.state}
            kind={urlState.kind ?? ""}
            hasFilters={hasFilters}
            onStateChange={(state) => void setUrlState({ state, page: null })}
            onKindChange={(kind) =>
              void setUrlState({ kind: kind === "" ? null : kind, page: null })
            }
            onClear={clearFilters}
          />
        }
        footer={
          <Pagination
            label="Jobs pages"
            position={{ kind: "offset", offset: (page - 1) * pageSize, pageSize, total }}
            onPrevious={page > 1 ? () => void setUrlState({ page: page - 1 }) : undefined}
            onNext={page < pageCount ? () => void setUrlState({ page: page + 1 }) : undefined}
          >
            {truncatedNotice ? "Within the 1000 most recently updated jobs" : undefined}
          </Pagination>
        }
      >
        <JobsTable
          items={items}
          isLoading={listQuery.isLoading}
          hasFilters={hasFilters}
          onClearFilters={clearFilters}
          onRowClick={openDetail}
          onRequeueClick={openConfirm}
          requeuePending={requeueMutation.isPending}
        />
      </ListSurface>

      <QuickDetailDrawer
        open={detailJob !== null}
        onOpenChange={(open) => !open && closeDetail()}
        title={detailJob?.kind ?? "Job"}
        description={detailJob != null ? `Job ${detailJob.id}` : undefined}
        footer={
          detailJob != null && detailJob.state === "dead" ? (
            <Button
              type="button"
              variant="secondary"
              size="sm"
              disabled={requeueMutation.isPending}
              onClick={() => openConfirm(detailJob)}
            >
              Requeue
            </Button>
          ) : undefined
        }
      >
        {detailJob != null && <JobDetailFields job={detailJob} />}
      </QuickDetailDrawer>

      <RequeueConfirmDialog
        job={confirmJob}
        pending={requeueMutation.isPending}
        onOpenChange={(open) => !open && closeConfirm()}
        onConfirm={confirmRequeue}
      />
    </ScreenStack>
  );
}
