// Dumb component (R6): the answer. A summary line the screen reader hears
// change, then one row per message. It renders what it is handed and shows the
// server's masked recipient only; it is not given the typed value, so it
// cannot repeat it.
//
// A row opens the quick-detail drawer. It does not link to `/messages/[id]`:
// the person this page is for (support) may not read messages, and a link to a
// page that refuses them would be a dead end.

import type { inferRouterOutputs } from "@trpc/server";
import {
  IdDisplay,
  InlineEmptyState,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
} from "@vaam-apps/ui";
import type { AppRouter } from "@vsms/api";
import { ChevronRight } from "lucide-react";
import { ListSurface } from "@/components/list-surface";
import { StatusPill } from "@/components/status";
import {
  emptyCaveat,
  outcomeLabel,
  PURGED_LABEL,
  resultsSummary,
  truncatedNote,
} from "../lookup-labels";
import type { LookupKind } from "../lookup-query";

type RouterOutputs = inferRouterOutputs<AppRouter>;
export type LookupResult = RouterOutputs["deliveryStatus"]["lookup"];
export type LookupStatus = LookupResult["statuses"][number];

const SKELETON_ROWS = 3;
const COLUMN_COUNT = 10;

export interface LookupResultsProps {
  result: LookupResult | undefined;
  isPending: boolean;
  kind: LookupKind;
  /** App names by id; an id it lacks is shown as the id. */
  appNames: Readonly<Record<string, string>>;
  onOpen: (messageId: string) => void;
}

/** The recipient cell: the masked number, or the word that says there is none. */
export function RecipientText({ status }: { status: LookupStatus }) {
  if (status.purged || status.recipient === undefined) {
    return <span className="text-muted-foreground italic">{PURGED_LABEL}</span>;
  }
  return <span className="font-mono">{status.recipient}</span>;
}

export function LookupResults({ result, isPending, kind, appNames, onOpen }: LookupResultsProps) {
  const statuses = result?.statuses ?? [];
  return (
    <section aria-busy={isPending}>
      <ListSurface
        toolbar={
          <p aria-live="polite" className="text-body-medium text-foreground">
            {isPending ? "Looking up…" : resultsSummary(statuses.length)}
          </p>
        }
        footer={
          !isPending && result?.truncated === true ? truncatedNote(statuses.length) : undefined
        }
      >
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Recipient</TableHead>
              <TableHead>State</TableHead>
              <TableHead hideBelow="md">App</TableHead>
              <TableHead hideBelow="md">Sender</TableHead>
              <TableHead hideBelow="lg">Operator</TableHead>
              <TableHead hideBelow="lg">Class</TableHead>
              <TableHead hideBelow="sm">Accepted</TableHead>
              <TableHead hideBelow="lg">Finalized</TableHead>
              <TableHead hideBelow="md">Last receipt</TableHead>
              <TableHead align="end">
                <span className="sr-only">Details</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {isPending &&
              Array.from({ length: SKELETON_ROWS }).map((_, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered
                <TableRow key={i}>
                  <TableCell colSpan={COLUMN_COUNT}>
                    <Skeleton className="h-4 w-full" />
                  </TableCell>
                </TableRow>
              ))}
            {!isPending && statuses.length === 0 && (
              <tr>
                <td colSpan={COLUMN_COUNT}>
                  <InlineEmptyState message={emptyCaveat(kind)} />
                </td>
              </tr>
            )}
            {!isPending &&
              statuses.map((status) => (
                <TableRow
                  key={status.messageId}
                  tabIndex={0}
                  role="button"
                  aria-label={`View details for message ${status.messageId}`}
                  className="cursor-pointer"
                  onClick={() => onOpen(status.messageId)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      onOpen(status.messageId);
                    }
                  }}
                >
                  <TableCell>
                    <RecipientText status={status} />
                  </TableCell>
                  <TableCell>
                    <StatusPill state={status.state} size="sm" />
                  </TableCell>
                  <TableCell hideBelow="md">
                    {appNames[status.appId] ?? <IdDisplay value={status.appId} />}
                  </TableCell>
                  <TableCell hideBelow="md" mono>
                    {status.senderIdValue}
                  </TableCell>
                  <TableCell hideBelow="lg" mono>
                    {status.operator}
                  </TableCell>
                  <TableCell hideBelow="lg" mono>
                    {status.class}
                  </TableCell>
                  <TableCell hideBelow="sm">
                    <TimestampDisplay value={status.acceptedAt} />
                  </TableCell>
                  <TableCell hideBelow="lg">
                    {status.finalizedAt !== undefined ? (
                      <TimestampDisplay value={status.finalizedAt} />
                    ) : (
                      "—"
                    )}
                  </TableCell>
                  <TableCell hideBelow="md">
                    {status.lastReceipt !== undefined ? (
                      <span className="inline-flex flex-wrap items-center gap-x-1.5">
                        <span>{outcomeLabel(status.lastReceipt.outcome)}</span>
                        <span aria-hidden="true">·</span>
                        <span className="font-mono text-muted-foreground">
                          {status.lastReceipt.rawStatus}
                        </span>
                        <span aria-hidden="true">·</span>
                        <TimestampDisplay value={status.lastReceipt.receivedAt} />
                      </span>
                    ) : (
                      "—"
                    )}
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
      </ListSurface>
    </section>
  );
}
