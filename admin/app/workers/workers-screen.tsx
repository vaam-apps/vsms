"use client";

// The Workers screen (#57): which node holds which singleton-role advisory
// lock. "`pg_locks` joined against the role-key table answers 'is dispatch
// running, and where' without shelling into a box" — the issue's own
// words.
//
// # What this screen can and cannot prove
//
// `crates/sms-api/src/worker_locks.rs`'s own module doc records what was
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

import { trpc } from "@vsms/hooks";
import {
  Badge,
  InlineEmptyState,
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

export function WorkersScreen() {
  const locksQuery = trpc.workers.locks.useQuery(undefined, {
    refetchInterval: REFETCH_INTERVAL_MS,
  });

  return (
    <main className="mx-auto flex max-w-[900px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Workers</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Which node holds which singleton-role advisory lock. Refreshes every{" "}
            {Math.round(REFETCH_INTERVAL_MS / 1000)}s.
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
            href="/jobs"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Jobs
          </a>
          <a
            href="/simulator"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Simulator
          </a>
          <ThemeToggle />
        </div>
      </header>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        A two-key advisory lock can never be held by two sessions at once — Postgres itself prevents
        that. This screen can show "unheld when it should be held," never "two holders." See{" "}
        <span className="font-mono text-foreground">crates/sms-api/src/worker_locks.rs</span> for
        what was verified live.
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
            <TableHead>Node</TableHead>
            <TableHead>Pid</TableHead>
            <TableHead align="end">Held since</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {locksQuery.isLoading &&
            Array.from({ length: 6 }).map((_, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
              <TableRow key={i}>
                <TableCell colSpan={5}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              </TableRow>
            ))}

          {!locksQuery.isLoading && (locksQuery.data?.locks.length ?? 0) === 0 && (
            <tr>
              <td colSpan={5}>
                <InlineEmptyState message="No role data returned — this is unexpected; all six roles should always be present." />
              </td>
            </tr>
          )}

          {locksQuery.data?.locks.map((lock) => (
            <TableRow key={lock.role}>
              <TableCell mono>{ROLE_LABELS[lock.role] ?? lock.role}</TableCell>
              <TableCell>
                {!lock.singleton ? (
                  <Badge variant="outline">scale-to-N — no lock</Badge>
                ) : lock.held ? (
                  <span className="rounded-sm border border-state-success-border bg-state-success-bg px-1.5 py-0.5 text-caption text-state-success-fg">
                    held
                  </span>
                ) : (
                  <span className="rounded-sm border border-state-danger-border bg-state-danger-bg px-1.5 py-0.5 text-caption text-state-danger-fg">
                    unheld
                  </span>
                )}
              </TableCell>
              <TableCell mono>{lock.workerId ?? "—"}</TableCell>
              <TableCell mono>{lock.pid ?? "—"}</TableCell>
              <TableCell align="end">
                {lock.heldSince != null ? <TimestampDisplay value={lock.heldSince} /> : "—"}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </main>
  );
}
