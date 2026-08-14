import {
  AttemptStatusPill,
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
import type { AttemptListItem } from "../webhook-domain";

// Dumb (R6): the delivery-attempts table. `endpointUrlFor` is a small
// display-lookup callback the screen already computes from its own
// endpoints query — passed in rather than duplicated here.
export function AttemptsTable({
  attempts,
  isLoading,
  endpointUrlFor,
  onRowClick,
}: {
  attempts: AttemptListItem[] | undefined;
  isLoading: boolean;
  endpointUrlFor: (endpointId: string) => string;
  onRowClick: (attempt: AttemptListItem) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>State</TableHead>
          <TableHead hideBelow="md">Event</TableHead>
          <TableHead hideBelow="lg">Endpoint</TableHead>
          <TableHead align="end" hideBelow="sm">
            Attempts
          </TableHead>
          <TableHead hideBelow="sm">Status</TableHead>
          <TableHead hideBelow="md">Last attempt</TableHead>
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

        {!isLoading && (attempts?.length ?? 0) === 0 && (
          <tr>
            <td colSpan={6}>
              <InlineEmptyState message="No delivery attempts match the current filters." />
            </td>
          </tr>
        )}

        {attempts?.map((attempt) => (
          <TableRow key={attempt.id} className="cursor-pointer" onClick={() => onRowClick(attempt)}>
            <TableCell>
              <AttemptStatusPill state={attempt.state} />
            </TableCell>
            <TableCell mono hideBelow="md">
              {attempt.eventType}
            </TableCell>
            <TableCell mono hideBelow="lg">
              <span
                className="line-clamp-1 max-w-[220px]"
                title={endpointUrlFor(attempt.endpointId)}
              >
                {endpointUrlFor(attempt.endpointId)}
              </span>
            </TableCell>
            <TableCell mono align="end" hideBelow="sm">
              {attempt.attempts}
            </TableCell>
            <TableCell mono hideBelow="sm">
              {attempt.lastStatusCode ?? "—"}
            </TableCell>
            <TableCell hideBelow="md">
              {attempt.lastAttemptAt != null ? (
                <TimestampDisplay value={attempt.lastAttemptAt} />
              ) : (
                <span className="text-muted-foreground">never</span>
              )}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
