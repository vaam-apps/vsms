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
// would be the diagnostic signal, not two rows on this screen. The banner
// below says this plainly rather than leaving an operator to assume the
// screen would catch that shape of bug directly.
//
// Only the four singleton roles (`dispatch`/`drain`/`scheduler`/`smpp`)
// ever hold this lock — `hooks`/`jobs` run scale-to-N and never call
// `pg_try_advisory_lock` at all. Both are still shown, clearly marked, so
// their permanent `held: false` doesn't read as a problem.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { trpc } from "@vsms/hooks";
import {
  Badge,
  InlineEmptyState,
  QuickDetailDrawer,
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
import { type ReactNode, useState } from "react";

type RouterOutputs = inferRouterOutputs<AppRouter>;
type WorkerLockInfo = RouterOutputs["workers"]["locks"]["locks"][number];

/** A diagnostics snapshot, not a live feed — a lease is held for the
 * lifetime of a worker process (hours to days), so polling faster than
 * this buys nothing; slower would make a `kill -9` failover (~5s per
 * AGENTS.md's own lease.rs findings) take a visibly stale screen to catch
 * up. */
const REFETCH_INTERVAL_MS = 5000;

const ROLE_LABELS: Record<string, string> = {
  dispatch: "Dispatch",
  drain: "Drain",
  scheduler: "Scheduler",
  hooks: "Hooks",
  jobs: "Jobs",
  smpp: "SMPP",
};

function StatusIndicator({ lock }: { lock: WorkerLockInfo }) {
  if (!lock.singleton) {
    return <Badge variant="outline">scale-to-N — no lock</Badge>;
  }
  if (lock.held) {
    return (
      <span className="rounded-sm border border-state-success-border bg-state-success-bg px-1.5 py-0.5 text-caption text-state-success-fg">
        held
      </span>
    );
  }
  return (
    <span className="rounded-sm border border-state-danger-border bg-state-danger-bg px-1.5 py-0.5 text-caption text-state-danger-fg">
      unheld
    </span>
  );
}

function WorkerDetailField({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5 border-edge-subtle border-b py-2 last:border-b-0">
      <dt className="text-caption text-subtle-foreground">{label}</dt>
      <dd className="text-body text-foreground">{value}</dd>
    </div>
  );
}

export function WorkersScreen() {
  const locksQuery = trpc.workers.locks.useQuery(undefined, {
    refetchInterval: REFETCH_INTERVAL_MS,
  });
  const [detailRole, setDetailRole] = useState<string | null>(null);

  const liveDetailLock =
    detailRole === null
      ? null
      : (locksQuery.data?.locks.find((lock) => lock.role === detailRole) ?? null);

  return (
    <div className="flex flex-col gap-6">
      <header className="flex flex-col gap-1">
        <h1 className="font-medium text-foreground text-title">Workers</h1>
        <p className="max-w-xl text-body text-muted-foreground">
          Which node holds which singleton-role advisory lock. Refreshes every{" "}
          {Math.round(REFETCH_INTERVAL_MS / 1000)}s.
        </p>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        A two-key advisory lock can never be held by two sessions at once — Postgres itself prevents
        that. This screen can show "unheld when it should be held," never "two holders." See{" "}
        <span className="font-mono text-foreground">
          backends/crates/sms-api/src/worker_locks.rs
        </span>{" "}
        for what was verified live.
      </div>

      {locksQuery.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read worker locks: {locksQuery.error.message}
        </div>
      )}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Role</TableHead>
            <TableHead>Status</TableHead>
            <TableHead className="hidden md:table-cell">Node</TableHead>
            <TableHead className="hidden lg:table-cell">Pid</TableHead>
            <TableHead align="end" className="hidden sm:table-cell">
              Held since
            </TableHead>
            <TableHead align="end" className="w-8" />
          </TableRow>
        </TableHeader>
        <TableBody>
          {locksQuery.isLoading &&
            Array.from({ length: 6 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={6}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!locksQuery.isLoading && (locksQuery.data?.locks.length ?? 0) === 0 && (
            <tr>
              <td colSpan={6}>
                <InlineEmptyState message="No role data returned — this is unexpected; all six roles should always be present." />
              </td>
            </tr>
          )}

          {locksQuery.data?.locks.map((lock) => (
            <TableRow
              key={lock.role}
              tabIndex={0}
              role="button"
              aria-label={`View details for role ${lock.role}`}
              className="cursor-pointer"
              onClick={() => setDetailRole(lock.role)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setDetailRole(lock.role);
                }
              }}
            >
              <TableCell mono>{ROLE_LABELS[lock.role] ?? lock.role}</TableCell>
              <TableCell>
                <StatusIndicator lock={lock} />
              </TableCell>
              <TableCell mono className="hidden max-w-[200px] truncate md:table-cell">
                {lock.workerId ?? "—"}
              </TableCell>
              <TableCell mono className="hidden lg:table-cell">
                {lock.pid ?? "—"}
              </TableCell>
              <TableCell align="end" className="hidden sm:table-cell">
                {lock.heldSince != null ? <TimestampDisplay value={lock.heldSince} /> : "—"}
              </TableCell>
              <TableCell align="end">
                <ChevronRight
                  size={14}
                  strokeWidth={1.5}
                  aria-hidden="true"
                  className="text-subtle-foreground"
                />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <QuickDetailDrawer
        open={liveDetailLock !== null}
        onOpenChange={(open) => !open && setDetailRole(null)}
        title={
          liveDetailLock != null
            ? (ROLE_LABELS[liveDetailLock.role] ?? liveDetailLock.role)
            : "Role"
        }
        description={
          liveDetailLock != null
            ? liveDetailLock.singleton
              ? "Singleton role — one lease at a time."
              : "Scale-to-N role — never takes this lock."
            : undefined
        }
      >
        {liveDetailLock != null && (
          <dl className="flex flex-col">
            <WorkerDetailField
              label="Role"
              value={<span className="font-mono">{liveDetailLock.role}</span>}
            />
            <WorkerDetailField label="Status" value={<StatusIndicator lock={liveDetailLock} />} />
            <WorkerDetailField
              label="Cardinality"
              value={
                liveDetailLock.singleton
                  ? "Singleton (one lease at a time)"
                  : "Scale-to-N (no lease)"
              }
            />
            <WorkerDetailField
              label="Worker id"
              value={
                liveDetailLock.workerId != null ? (
                  <span className="break-all font-mono">{liveDetailLock.workerId}</span>
                ) : (
                  "—"
                )
              }
            />
            <WorkerDetailField
              label="Postgres pid"
              value={
                liveDetailLock.pid != null ? (
                  <span className="font-mono">{liveDetailLock.pid}</span>
                ) : (
                  "—"
                )
              }
            />
            <WorkerDetailField
              label="Held since"
              value={
                liveDetailLock.heldSince != null ? (
                  <TimestampDisplay value={liveDetailLock.heldSince} />
                ) : (
                  "—"
                )
              }
            />
          </dl>
        )}
      </QuickDetailDrawer>
    </div>
  );
}
