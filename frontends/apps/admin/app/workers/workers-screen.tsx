"use client";

// The Workers screen (#57): which node holds which singleton-role advisory
// lock. "`pg_locks` joined against the role-key table answers 'is dispatch
// running, and where' without shelling into a box" — the issue's own
// words.
//
// Console redesign (docs/design/console-redesign.md, Phase 2
// "Operations"): dropped the old per-screen hand-rolled `<header>` nav
// block for `ConsoleShell`'s own sidebar/top bar, and added a row-click
// `QuickDetailDrawer` (§3/D14) — the table itself only shows Role/Status
// at phone width (`workerId`/`pid` can be long `hostname:pid`-shaped
// strings that don't fit six-to-a-row on a 375px screen), with the full
// row one tap away. §3's own per-screen list doesn't name Workers
// explicitly among the Quick-detail candidates, but the same "which
// columns move into the drawer" reasoning §8's risk list applies to every
// dense table applies here too — this is a small, fixed six-row table, not
// a paginated list, so the drawer is a peek, never a destination.
//
// # What this screen can and cannot prove
//
// `backends/crates/sms-api/src/worker_locks.rs`'s own module doc records what was
// verified live against a real Postgres, not assumed: a two-key advisory
// lock is exclusive by construction, so **this table can never show two
// held rows for the same role** — Postgres's own locking guarantees that,
// the same guarantee that makes `RoleLease` safe leader election in the
// first place. If two processes were ever genuinely both acting as
// `dispatch`, that would be a bug bypassing the lease mechanism entirely,
// and this screen would still show exactly one holder — the mismatch
// between "one holder here" and "double-submission observed elsewhere"
// would be the diagnostic signal, not two rows on this screen. The caption
// under the role cards says this plainly rather than leaving an operator to assume the
// screen would catch that shape of bug directly.
//
// Only the four singleton roles (`dispatch`/`drain`/`scheduler`/`smpp`)
// ever hold this lock — `hooks`/`jobs` run scale-to-N and never call
// `pg_try_advisory_lock` at all. Both are still shown, clearly marked, so
// their permanent `held: false` doesn't read as a problem.
//
// The six roles render as cards (`components/worker-role-grid.tsx`), not a
// table: a fixed set of six whose node and lease age should be visible at
// every width, which the old table's hidden-below-md columns were not.
//
// R6 (AGENTS.md): this file is the smart component. Markup and classes
// live in `components/worker-role-grid.tsx`, `components/worker-status.tsx`
// (`StatusIndicator`) and `components/worker-detail-fields.tsx`
// (`WorkerDetailField` included); `ROLE_LABELS` moved into
// `role-labels.ts`, a pure module with its own test (`role-labels.test.ts`)
// per R6's own rule for extracted pure modules. `REFETCH_INTERVAL_MS` is
// gone — `pollMs` now arrives as a prop from `page.tsx`, reading
// `@vsms/env`'s `DIAGNOSTICS_POLL_MS` server-side, the same value
// `jobs-screen.tsx` reads (both screens polled the identical 5000ms
// independently before this). The `detailRole` drawer target moved from
// `useState` into the URL (`nuqs`, `history: "replace"` so opening/closing
// the drawer doesn't grow a back-button trail) — it was already only a
// role string, never a copy of a fetched row, so it had none of
// `jobs-screen.tsx`'s staleness-fallback problem, but R6's own "URL/filter
// state → nuqs" guidance applies regardless: a screen showing "which role's
// drawer is open" is shareable/bookmarkable state, and moving it means this
// screen now needs its own `Suspense` boundary in `page.tsx` (previously
// none, since nothing here read `useSearchParams()`).

import { InlineBanner, QuickDetailDrawer, ScreenStack } from "@vaam-apps/ui";
import { trpc } from "@vsms/hooks";
import { parseAsString, useQueryState } from "nuqs";
import { Caption } from "@/components/caption";
import { LiveChip } from "@/components/live-chip";
import { PageHeader } from "@/components/page-header";
import {
  WorkerDetailFields,
  workerDetailDescription,
  workerDetailTitle,
} from "./components/worker-detail-fields";
import { WorkerRoleGrid } from "./components/worker-role-grid";
import type { WorkerLockInfo } from "./components/worker-status";

export interface WorkersScreenProps {
  /** `DIAGNOSTICS_POLL_MS`, read server-side (`page.tsx`) — see this file's
   * own module doc. */
  pollMs: number;
}

export function WorkersScreen({ pollMs }: WorkersScreenProps) {
  const locksQuery = trpc.workers.locks.useQuery(undefined, {
    refetchInterval: pollMs,
  });
  const [detailRole, setDetailRole] = useQueryState(
    "detail",
    parseAsString.withOptions({ history: "replace" }),
  );

  const locks: WorkerLockInfo[] = locksQuery.data?.locks ?? [];
  const detailLock =
    detailRole === null ? null : (locks.find((lock) => lock.role === detailRole) ?? null);

  function openDetail(lock: WorkerLockInfo) {
    void setDetailRole(lock.role);
  }

  function closeDetail() {
    void setDetailRole(null);
  }

  return (
    <ScreenStack>
      <PageHeader
        title="Workers"
        description="Which node runs each worker role. Dispatch, drain, scheduler and SMPP run on one node at a time, under a lease."
        actions={
          <LiveChip
            live={!locksQuery.isError}
            label={`Updating every ${Math.round(pollMs / 1000)} s`}
          />
        }
      />

      {locksQuery.isError && (
        <InlineBanner variant="danger">
          Could not read worker locks: {locksQuery.error.message}
        </InlineBanner>
      )}

      <WorkerRoleGrid locks={locks} isLoading={locksQuery.isLoading} onOpen={openDetail} />

      <Caption>
        Postgres lets exactly one session hold a role's lease, so this screen can show a role unheld
        but never held twice.
      </Caption>

      <QuickDetailDrawer
        open={detailLock !== null}
        onOpenChange={(open) => !open && closeDetail()}
        title={workerDetailTitle(detailLock)}
        description={workerDetailDescription(detailLock)}
      >
        {detailLock != null && <WorkerDetailFields lock={detailLock} />}
      </QuickDetailDrawer>
    </ScreenStack>
  );
}
