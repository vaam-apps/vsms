import {
  IdDisplay,
  InlineEmptyState,
  Skeleton,
  StateChip,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TimestampDisplay,
} from "@vsms/ui";
import type { RouteListItem } from "../route-domain";
import { predicateSummary } from "../route-domain";

// Dumb, route-local (R6): the route list table — loading skeleton, empty
// state, and real rows. Knows nothing about where `routes` came from.
export function RouteTable({
  routes,
  isLoading,
  onRowClick,
}: {
  routes: RouteListItem[] | undefined;
  isLoading: boolean;
  onRowClick: (route: RouteListItem) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead align="end">Priority</TableHead>
          <TableHead align="end" hideBelow="sm">
            Weight
          </TableHead>
          <TableHead>Status</TableHead>
          <TableHead>Name</TableHead>
          <TableHead hideBelow="md">Predicates</TableHead>
          <TableHead hideBelow="lg">Provider</TableHead>
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

        {!isLoading && (routes?.length ?? 0) === 0 && (
          <tr>
            <td colSpan={7}>
              <InlineEmptyState message="No routes configured." />
            </td>
          </tr>
        )}

        {routes?.map((route) => (
          <TableRow key={route.id} className="cursor-pointer" onClick={() => onRowClick(route)}>
            <TableCell align="end" mono>
              {route.priority}
            </TableCell>
            <TableCell align="end" mono hideBelow="sm">
              {route.weight}
            </TableCell>
            <TableCell>
              {route.enabled ? (
                <StateChip tone="success">enabled</StateChip>
              ) : (
                <StateChip tone="danger">disabled</StateChip>
              )}
            </TableCell>
            <TableCell>{route.name}</TableCell>
            <TableCell hideBelow="md" className="text-caption text-muted-foreground">
              {predicateSummary(route)}
            </TableCell>
            <TableCell hideBelow="lg">
              <IdDisplay value={route.providerId} />
            </TableCell>
            <TableCell align="end" hideBelow="md">
              <TimestampDisplay value={route.updatedAt} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
