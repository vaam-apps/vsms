// Dumb view: the audit entries table. Loading/empty states and the row
// click callback are the only "logic" here — pure rendering of props.

import {
  InlineEmptyState,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
} from "@vsms/ui";
import type { AuditLogEntry } from "../types";

export function AuditLogTable({
  entries,
  isLoading,
  errorMessage,
  onRowClick,
}: {
  entries: AuditLogEntry[];
  isLoading: boolean;
  errorMessage: string | null;
  onRowClick: (entry: AuditLogEntry) => void;
}) {
  return (
    <div className="flex flex-col gap-3">
      {errorMessage !== null && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Could not read the audit log: {errorMessage}
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
          {isLoading && (
            <TableRow>
              <TableCell colSpan={5}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          )}
          {!isLoading && entries.length === 0 && (
            <TableRow>
              <TableCell colSpan={5}>
                <InlineEmptyState message="No matching audit entries." />
              </TableCell>
            </TableRow>
          )}
          {entries.map((entry) => (
            <TableRow
              key={entry.eventId}
              className="cursor-pointer"
              onClick={() => onRowClick(entry)}
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
    </div>
  );
}
