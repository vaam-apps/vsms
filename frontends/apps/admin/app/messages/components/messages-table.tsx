// Dumb — route-local to messages (R6). The list table itself: loading
// skeleton, empty state, and one `LiveRow` per message. Takes the rows it
// is handed and renders them; knows nothing about polling, filters as
// URL state, or where a row came from (an initial fetch vs. a live event).

import {
  IdDisplay,
  InlineEmptyState,
  LiveRow,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
} from "@vaam-apps/ui";
import { ChevronRight } from "lucide-react";
import { MsisdnDisplay } from "@/components/msisdn-display";
import { MESSAGE_STATUS_META, StatusPill } from "@/components/status";
import type { MessageListItem } from "../apply-event";

const SKELETON_ROW_COUNT = 8;
const COLUMN_COUNT = 7;

export interface MessagesTableProps {
  rows: MessageListItem[];
  /** Opens a message's detail page. The whole row is the target; the id
   * cell keeps its own copy button, which this does not swallow. */
  onOpen: (id: string) => void;
  isLoading: boolean;
  hasFilters: boolean;
  onClearFilters: () => void;
}

export function MessagesTable({
  rows,
  onOpen,
  isLoading,
  hasFilters,
  onClearFilters,
}: MessagesTableProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Status</TableHead>
          <TableHead>Recipient</TableHead>
          <TableHead hideBelow="md">Client ref</TableHead>
          <TableHead hideBelow="sm">Sender</TableHead>
          <TableHead hideBelow="lg">Encoding</TableHead>
          <TableHead hideBelow="sm">Id</TableHead>
          <TableHead align="end">Sent</TableHead>
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
            className="cursor-pointer"
            onClick={(event) => {
              // A click on the copy button or the link already did its job.
              if ((event.target as HTMLElement).closest("button, a") !== null) return;
              onOpen(row.id);
            }}
          >
            <TableCell>
              <StatusPill state={row.state} size="sm" />
            </TableCell>
            <TableCell>
              <MsisdnDisplay value={row.msisdn} operator={row.operator} />
              {/* Below `md`, the Client ref/Sender/Encoding columns are
               * hidden outright rather than horizontally scrolled to —
               * this compact line keeps that same information reachable
               * at a glance instead of losing it. Hidden again once those
               * columns return at `md`, so nothing renders twice. */}
              <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 font-mono text-label-small text-muted-foreground md:hidden">
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
            <TableCell hideBelow="sm">
              <IdDisplay value={row.id} />
            </TableCell>
            <TableCell align="end">
              <span className="inline-flex items-center gap-1">
                <TimestampDisplay value={row.createdAt} />
                {/* The keyboard and middle-click route to the detail page;
                 * the row's own click handler is the pointer shortcut. */}
                <a
                  href={`/messages/${row.id}`}
                  aria-label={`Open message ${row.id}`}
                  className="inline-flex size-8 items-center justify-center rounded-full text-muted-foreground hover:bg-foreground/8 hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary"
                >
                  <ChevronRight size={18} aria-hidden="true" />
                </a>
              </span>
            </TableCell>
          </LiveRow>
        ))}
      </TableBody>
    </Table>
  );
}
