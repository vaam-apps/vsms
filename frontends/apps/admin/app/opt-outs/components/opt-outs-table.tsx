// Dumb component (R6): markup, classes, and iteration over the rows it is
// handed. Moved verbatim out of `opt-outs-screen.tsx` — no data fetching,
// no tRPC.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import {
  Button,
  InlineEmptyState,
  MsisdnDisplay,
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
export type OptOutListItem = RouterOutputs["optOuts"]["list"][number];

export interface OptOutsTableProps {
  items: OptOutListItem[];
  isLoading: boolean;
  onRowClick: (row: OptOutListItem) => void;
  onRemoveClick: (row: OptOutListItem) => void;
}

export function OptOutsTable({ items, isLoading, onRowClick, onRemoveClick }: OptOutsTableProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>MSISDN</TableHead>
          <TableHead>Source</TableHead>
          <TableHead className="hidden md:table-cell">Scope</TableHead>
          <TableHead className="hidden lg:table-cell">Reason</TableHead>
          <TableHead align="end" className="hidden sm:table-cell">
            Opted out
          </TableHead>
          <TableHead align="end">Actions</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {isLoading && (
          <TableRow>
            <TableCell colSpan={6}>
              <Skeleton className="h-4 w-full" />
            </TableCell>
          </TableRow>
        )}
        {!isLoading && items.length === 0 && (
          <tr>
            <td colSpan={6}>
              <InlineEmptyState message="No opt-outs recorded yet." />
            </td>
          </tr>
        )}
        {items.map((row) => (
          <TableRow
            key={row.id}
            tabIndex={0}
            role="button"
            aria-label={`View details for opt-out ${row.msisdn}`}
            className="cursor-pointer"
            onClick={() => onRowClick(row)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onRowClick(row);
              }
            }}
          >
            <TableCell>
              <MsisdnDisplay value={row.msisdn} />
            </TableCell>
            <TableCell mono>{row.source}</TableCell>
            <TableCell mono className="hidden md:table-cell">
              {row.scope}
            </TableCell>
            <TableCell className="hidden max-w-[240px] truncate text-caption text-muted-foreground lg:table-cell">
              {row.reason ?? "—"}
            </TableCell>
            <TableCell align="end" className="hidden sm:table-cell">
              <TimestampDisplay value={row.optedOutAt} />
            </TableCell>
            <TableCell align="end">
              <div className="flex items-center justify-end gap-1.5">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={(e) => {
                    e.stopPropagation();
                    onRemoveClick(row);
                  }}
                >
                  Remove
                </Button>
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
