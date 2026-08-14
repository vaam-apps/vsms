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
          <TableHead align="end" className="hidden sm:table-cell">
            Weight
          </TableHead>
          <TableHead>Status</TableHead>
          <TableHead>Name</TableHead>
          <TableHead className="hidden md:table-cell">Predicates</TableHead>
          <TableHead className="hidden lg:table-cell">Provider</TableHead>
          <TableHead align="end" className="hidden md:table-cell">
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
            <TableCell align="end" mono className="hidden sm:table-cell">
              {route.weight}
            </TableCell>
            <TableCell>
              {route.enabled ? (
                <span className="rounded-sm border border-state-success-border bg-state-success-bg px-1.5 py-0.5 text-caption text-state-success-fg">
                  enabled
                </span>
              ) : (
                <span className="rounded-sm border border-state-danger-border bg-state-danger-bg px-1.5 py-0.5 text-caption text-state-danger-fg">
                  disabled
                </span>
              )}
            </TableCell>
            <TableCell>{route.name}</TableCell>
            <TableCell className="hidden text-caption text-muted-foreground md:table-cell">
              {predicateSummary(route)}
            </TableCell>
            <TableCell className="hidden lg:table-cell">
              <IdDisplay value={route.providerId} />
            </TableCell>
            <TableCell align="end" className="hidden md:table-cell">
              <TimestampDisplay value={route.updatedAt} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
