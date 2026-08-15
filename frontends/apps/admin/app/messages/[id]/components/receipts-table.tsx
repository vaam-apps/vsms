// Dumb — route-local to the message detail screen (R6). The raw
// `DeliveryReceipt` evidence list — every row this system has for this
// message, verbatim, including duplicates and out-of-order arrivals. See
// `message-detail-screen.tsx`'s own module doc for why this is shown
// separately from the timeline rather than folded into it.

import {
  InlineBanner,
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
  /** Set when `messages.receipts` itself failed — distinct from a
   * successful fetch that genuinely found zero rows (#311). Takes
   * priority over the empty state below: "couldn't load" and "none
   * recorded" must never render identically on the one screen whose
   * purpose is being the evidence trail for diagnosing a message. */
  errorMessage?: string | undefined;
}

export function ReceiptsTable({ receipts, isLoading, errorMessage }: ReceiptsTableProps) {
  if (isLoading) {
    return (
      <div className="flex flex-col gap-2">
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-full" />
      </div>
    );
  }

  if (errorMessage != null) {
    return (
      <InlineBanner variant="danger">Could not load delivery receipts: {errorMessage}</InlineBanner>
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
          <TableHead hideBelow="sm">Raw status</TableHead>
          <TableHead hideBelow="md">Error code</TableHead>
          <TableHead hideBelow="md">Network</TableHead>
          <TableHead align="end">Received</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {receipts.map((receipt) => (
          <TableRow key={receipt.id}>
            <TableCell mono>{receipt.outcome}</TableCell>
            <TableCell hideBelow="sm" mono>
              {receipt.rawStatus}
            </TableCell>
            <TableCell hideBelow="md" mono>
              {receipt.errorCode ?? "—"}
            </TableCell>
            <TableCell hideBelow="md" mono>
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
