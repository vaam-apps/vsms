// Dumb — route-local to messages (R6). The list table itself: loading
// skeleton, empty state, and one `LiveRow` per message. Takes the rows it
// is handed and renders them; knows nothing about polling, filters as
// URL state, or where a row came from (an initial fetch vs. a live event).

import {
  IdDisplay,
  InlineEmptyState,
  LiveRow,
  MESSAGE_STATUS_META,
  MsisdnDisplay,
  Skeleton,
  StatusPill,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
} from "@vsms/ui";
import type { MessageListItem } from "../apply-event";

const SKELETON_ROW_COUNT = 8;
const COLUMN_COUNT = 7;

export interface MessagesTableProps {
  rows: MessageListItem[];
  isLoading: boolean;
  hasFilters: boolean;
  onClearFilters: () => void;
}

export function MessagesTable({ rows, isLoading, hasFilters, onClearFilters }: MessagesTableProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Status</TableHead>
          <TableHead>Recipient</TableHead>
          <TableHead hideBelow="md">Client ref</TableHead>
          <TableHead hideBelow="sm">Sender</TableHead>
          <TableHead hideBelow="lg">Encoding</TableHead>
          <TableHead>Id</TableHead>
          <TableHead align="end">Time</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {isLoading &&
          Array.from({ length: SKELETON_ROW_COUNT }).map((_, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
            <TableRow key={i}>
              <TableCell colSpan={COLUMN_COUNT}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          ))}

        {!isLoading && rows.length === 0 && (
          <tr>
            <td colSpan={COLUMN_COUNT}>
              <InlineEmptyState
                message={hasFilters ? "No messages match the current filters." : "No messages yet."}
                {...(hasFilters
                  ? { action: { label: "Clear filters", onClick: onClearFilters } }
                  : {})}
              />
            </td>
          </tr>
        )}

        {rows.map((row) => (
          <LiveRow
            key={row.id}
            washTrigger={row.version}
            washHue={MESSAGE_STATUS_META[row.state].hue}
          >
            <TableCell>
              <StatusPill state={row.state} />
            </TableCell>
            <TableCell>
              <MsisdnDisplay value={row.msisdn} operator={row.operator} />
              {/* Below `md`, the Client ref/Sender/Encoding columns are
               * hidden outright rather than horizontally scrolled to —
               * this compact line keeps that same information reachable
               * at a glance instead of losing it. Hidden again once those
               * columns return at `md`, so nothing renders twice. */}
              <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 font-mono text-[11px] text-subtle-foreground md:hidden">
                <span>{row.senderIdValue}</span>
                <span aria-hidden="true">·</span>
                <span>
                  {row.encoding.toUpperCase()} {row.segments}
                </span>
                {row.clientRef != null && row.clientRef !== "" && (
                  <>
                    <span aria-hidden="true">·</span>
                    <span className="max-w-[160px] truncate">{row.clientRef}</span>
                  </>
                )}
              </div>
            </TableCell>
            <TableCell hideBelow="md" mono>
              {row.clientRef ?? "—"}
            </TableCell>
            <TableCell hideBelow="sm" mono>
              {row.senderIdValue}
            </TableCell>
            <TableCell hideBelow="lg" mono>
              {row.encoding.toUpperCase()} · {row.segments}
            </TableCell>
            <TableCell>
              <div className="flex items-center gap-2">
                <IdDisplay value={row.id} />
                {/* #50: the detail route. A plain `<a>`, matching the rest
                 * of this console's internal navigation — not `next/
                 * link`'s `Link`. Separate from `IdDisplay` itself rather
                 * than wrapping it: `IdDisplay`'s own copy button doesn't
                 * stop propagation, so wrapping it in an `<a>` would fire
                 * a navigation on every copy click. */}
                <a
                  href={`/messages/${row.id}`}
                  className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
                >
                  View
                </a>
              </div>
            </TableCell>
            <TableCell align="end">
              <TimestampDisplay value={row.createdAt} />
            </TableCell>
          </LiveRow>
        ))}
      </TableBody>
    </Table>
  );
}
