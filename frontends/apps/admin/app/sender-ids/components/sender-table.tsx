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
import type { SenderIdListItem } from "../sender-id-domain";

// Dumb (R6): the sender ID list table. `summaryFor` is a small
// display-lookup callback the screen already computes from its own
// registrations query — passed in rather than duplicated here.
export function SenderTable({
  senderIds,
  isLoading,
  summaryFor,
  onRowClick,
}: {
  senderIds: SenderIdListItem[] | undefined;
  isLoading: boolean;
  summaryFor: (senderIdId: string) => string;
  onRowClick: (senderId: SenderIdListItem) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Active</TableHead>
          <TableHead>Value</TableHead>
          <TableHead hideBelow="sm">Kind</TableHead>
          <TableHead hideBelow="md">Registrations</TableHead>
          <TableHead align="end" hideBelow="md">
            Updated
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {isLoading &&
          Array.from({ length: 3 }).map((_, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
            <TableRow key={i}>
              <TableCell colSpan={5}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          ))}

        {!isLoading && (senderIds?.length ?? 0) === 0 && (
          <tr>
            <td colSpan={5}>
              <InlineEmptyState message="No sender IDs configured yet." />
            </td>
          </tr>
        )}

        {senderIds?.map((senderId) => (
          <TableRow
            key={senderId.id}
            className="cursor-pointer"
            onClick={() => onRowClick(senderId)}
          >
            <TableCell>
              {senderId.active ? (
                <span className="text-state-success-fg">active</span>
              ) : (
                <span className="text-muted-foreground">inactive</span>
              )}
            </TableCell>
            <TableCell mono>{senderId.value}</TableCell>
            <TableCell mono hideBelow="sm">
              {senderId.kind}
            </TableCell>
            <TableCell hideBelow="md">
              <span className="text-caption text-muted-foreground">{summaryFor(senderId.id)}</span>
            </TableCell>
            <TableCell align="end" hideBelow="md">
              <TimestampDisplay value={senderId.updatedAt} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
