// Dumb component (R6): the Providers list table, its loading skeleton, and
// its empty state. Markup moved verbatim out of `providers-screen.tsx`;
// `onRowClick` is the only behaviour it's handed — opening the quick-detail
// drawer stays the smart component's call.

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
import type { ProviderState } from "../provider-types";
import { StatePill } from "./state-pill";

export interface ProviderRow {
  id: string;
  state: ProviderState;
  displayName: string;
  key: string;
  kind: string;
  healthy: boolean;
  maxTps: number;
  costPerSegmentXaf: string;
  updatedAt: string;
}

export interface ProvidersTableProps {
  rows: ProviderRow[];
  isLoading: boolean;
  onRowClick: (id: string) => void;
}

export function ProvidersTable({ rows, isLoading, onRowClick }: ProvidersTableProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>State</TableHead>
          <TableHead>Provider</TableHead>
          <TableHead hideBelow="md">Kind</TableHead>
          <TableHead hideBelow="sm">Healthy</TableHead>
          <TableHead align="end" hideBelow="sm">
            Max TPS
          </TableHead>
          <TableHead align="end" hideBelow="lg">
            Cost/segment (XAF)
          </TableHead>
          <TableHead align="end" hideBelow="md">
            Updated
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {isLoading &&
          Array.from({ length: 4 }).map((_, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: static skeleton rows, never reordered or diffed
            <TableRow key={i}>
              <TableCell colSpan={7}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          ))}

        {!isLoading && rows.length === 0 && (
          <tr>
            <td colSpan={7}>
              <InlineEmptyState message="No providers configured yet." />
            </td>
          </tr>
        )}

        {rows.map((provider) => (
          <TableRow
            key={provider.id}
            className="cursor-pointer"
            onClick={() => onRowClick(provider.id)}
          >
            <TableCell>
              <StatePill state={provider.state} />
            </TableCell>
            <TableCell>
              <div className="flex flex-col">
                <span>{provider.displayName}</span>
                <span className="font-mono text-caption text-subtle-foreground">
                  {provider.key}
                </span>
              </div>
            </TableCell>
            <TableCell mono hideBelow="md">
              {provider.kind}
            </TableCell>
            <TableCell hideBelow="sm">
              {provider.healthy ? (
                <span className="text-state-success-fg">yes</span>
              ) : (
                <span className="text-muted-foreground">no probe yet</span>
              )}
            </TableCell>
            <TableCell align="end" mono hideBelow="sm">
              {provider.maxTps}
            </TableCell>
            <TableCell align="end" mono hideBelow="lg">
              {provider.costPerSegmentXaf}
            </TableCell>
            <TableCell align="end" hideBelow="md">
              <TimestampDisplay value={provider.updatedAt} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
