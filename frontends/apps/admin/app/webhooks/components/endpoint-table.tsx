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
import { type EndpointListItem, EVENT_TYPES } from "../webhook-domain";

function isCircuitOpen(endpoint: Pick<EndpointListItem, "circuitOpenUntil">): boolean {
  return endpoint.circuitOpenUntil != null && new Date(endpoint.circuitOpenUntil) > new Date();
}

// Dumb (R6): the endpoint list table.
export function EndpointTable({
  endpoints,
  isLoading,
  onRowClick,
}: {
  endpoints: EndpointListItem[] | undefined;
  isLoading: boolean;
  onRowClick: (endpoint: EndpointListItem) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Active</TableHead>
          <TableHead>URL</TableHead>
          <TableHead className="hidden md:table-cell">Events</TableHead>
          <TableHead className="hidden sm:table-cell">Circuit</TableHead>
          <TableHead align="end" className="hidden md:table-cell">
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

        {!isLoading && (endpoints?.length ?? 0) === 0 && (
          <tr>
            <td colSpan={5}>
              <InlineEmptyState message="No webhook endpoints configured yet." />
            </td>
          </tr>
        )}

        {endpoints?.map((endpoint) => {
          const circuitOpen = isCircuitOpen(endpoint);
          return (
            <TableRow
              key={endpoint.id}
              className="cursor-pointer"
              onClick={() => onRowClick(endpoint)}
            >
              <TableCell>
                {endpoint.active ? (
                  <span className="text-state-success-fg">active</span>
                ) : (
                  <span className="text-muted-foreground">inactive</span>
                )}
              </TableCell>
              <TableCell mono>
                <span className="line-clamp-1 max-w-[220px] sm:max-w-[320px]" title={endpoint.url}>
                  {endpoint.url}
                </span>
              </TableCell>
              <TableCell className="hidden md:table-cell">
                <span className="text-caption text-muted-foreground">
                  {endpoint.eventTypes.length} of {EVENT_TYPES.length}
                </span>
              </TableCell>
              <TableCell className="hidden sm:table-cell">
                {circuitOpen ? (
                  <span
                    className="text-state-danger-fg"
                    title={`Open until ${endpoint.circuitOpenUntil}`}
                  >
                    open ({endpoint.consecutiveFailures})
                  </span>
                ) : (
                  <span className="text-muted-foreground">closed</span>
                )}
              </TableCell>
              <TableCell align="end" className="hidden md:table-cell">
                <TimestampDisplay value={endpoint.updatedAt} />
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}
