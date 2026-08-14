// Dumb component (R6): markup, classes, and iteration over the rows it is
// handed. Moved verbatim out of `workers-screen.tsx` (`StatusIndicator`
// included) — no data fetching, no tRPC.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import {
  Badge,
  InlineEmptyState,
  Skeleton,
  StateChip,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
} from "@vsms/ui";
import { ChevronRight } from "lucide-react";
import { roleLabel } from "../role-labels";

type RouterOutputs = inferRouterOutputs<AppRouter>;
export type WorkerLockInfo = RouterOutputs["workers"]["locks"]["locks"][number];

function StatusIndicator({ lock }: { lock: WorkerLockInfo }) {
  if (!lock.singleton) {
    return <Badge variant="outline">scale-to-N — no lock</Badge>;
  }
  if (lock.held) {
    return <StateChip tone="success">held</StateChip>;
  }
  return <StateChip tone="danger">unheld</StateChip>;
}

export interface WorkersTableProps {
  locks: WorkerLockInfo[];
  isLoading: boolean;
  onRowClick: (lock: WorkerLockInfo) => void;
}

export function WorkersTable({ locks, isLoading, onRowClick }: WorkersTableProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Role</TableHead>
          <TableHead>Status</TableHead>
          <TableHead hideBelow="md">Node</TableHead>
          <TableHead hideBelow="lg">Pid</TableHead>
          <TableHead align="end" hideBelow="sm">
            Held since
          </TableHead>
          <TableHead align="end" className="w-8" />
        </TableRow>
      </TableHeader>
      <TableBody>
        {isLoading &&
          Array.from({ length: 6 }).map((_, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
            <TableRow key={i}>
              <TableCell colSpan={6}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          ))}

        {!isLoading && locks.length === 0 && (
          <tr>
            <td colSpan={6}>
              <InlineEmptyState message="No role data returned — this is unexpected; all six roles should always be present." />
            </td>
          </tr>
        )}

        {locks.map((lock) => (
          <TableRow
            key={lock.role}
            tabIndex={0}
            role="button"
            aria-label={`View details for role ${lock.role}`}
            className="cursor-pointer"
            onClick={() => onRowClick(lock)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onRowClick(lock);
              }
            }}
          >
            <TableCell mono>{roleLabel(lock.role)}</TableCell>
            <TableCell>
              <StatusIndicator lock={lock} />
            </TableCell>
            <TableCell mono hideBelow="md" className="max-w-[200px] truncate">
              {lock.workerId ?? "—"}
            </TableCell>
            <TableCell mono hideBelow="lg">
              {lock.pid ?? "—"}
            </TableCell>
            <TableCell align="end" hideBelow="sm">
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
  );
}

export { StatusIndicator };
