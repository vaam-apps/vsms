// Dumb — route-local to the message detail screen (R6). The raw
// `DeliveryReceipt` evidence list — every row this system has for this
// message, verbatim, including duplicates and out-of-order arrivals. See
// `message-detail-screen.tsx`'s own module doc for why this is shown
// separately from the timeline rather than folded into it.

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
import type { DeliveryReceiptSummary } from "../message-record";

export interface ReceiptsTableProps {
  receipts: DeliveryReceiptSummary[] | undefined;
  isLoading: boolean;
}

export function ReceiptsTable({ receipts, isLoading }: ReceiptsTableProps) {
  if (isLoading) {
    return (
      <div className="flex flex-col gap-2">
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-full" />
      </div>
    );
  }

  if (receipts === undefined || receipts.length === 0) {
    return (
      <InlineEmptyState message="No delivery receipts recorded for this message — see the note above the timeline for what that does and doesn't mean." />
    );
  }

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Outcome</TableHead>
          <TableHead className="hidden sm:table-cell">Raw status</TableHead>
          <TableHead className="hidden md:table-cell">Error code</TableHead>
          <TableHead className="hidden md:table-cell">Network</TableHead>
          <TableHead align="end">Received</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {receipts.map((receipt) => (
          <TableRow key={receipt.id}>
            <TableCell mono>{receipt.outcome}</TableCell>
            <TableCell className="hidden sm:table-cell" mono>
              {receipt.rawStatus}
            </TableCell>
            <TableCell className="hidden md:table-cell" mono>
              {receipt.errorCode ?? "—"}
            </TableCell>
            <TableCell className="hidden md:table-cell" mono>
              {receipt.networkCode}
            </TableCell>
            <TableCell align="end">
              <TimestampDisplay value={receipt.receivedAt} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
