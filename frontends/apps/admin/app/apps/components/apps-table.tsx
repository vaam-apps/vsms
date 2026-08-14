// Dumb view: the apps list table.

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
import type { AppListItem } from "../types";

export function AppsTable({
  apps,
  isLoading,
  onRowClick,
}: {
  apps: AppListItem[];
  isLoading: boolean;
  onRowClick: (app: AppListItem) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Active</TableHead>
          <TableHead>Name</TableHead>
          <TableHead className="hidden sm:table-cell">Slug</TableHead>
          <TableHead align="end" className="hidden md:table-cell">
            Monthly quota
          </TableHead>
          <TableHead className="hidden lg:table-cell">Transliterate to GSM-7</TableHead>
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
              <TableCell colSpan={6}>
                <Skeleton className="h-4 w-full" />
              </TableCell>
            </TableRow>
          ))}

        {!isLoading && apps.length === 0 && (
          <TableRow>
            <TableCell colSpan={6}>
              <InlineEmptyState message="No apps yet." />
            </TableCell>
          </TableRow>
        )}

        {apps.map((app) => (
          <TableRow key={app.id} className="cursor-pointer" onClick={() => onRowClick(app)}>
            <TableCell>
              {app.active ? (
                <span className="text-state-success-fg">yes</span>
              ) : (
                <span className="text-muted-foreground">no</span>
              )}
            </TableCell>
            <TableCell>{app.name}</TableCell>
            <TableCell mono className="hidden sm:table-cell">
              {app.slug}
            </TableCell>
            <TableCell align="end" mono className="hidden md:table-cell">
              {app.monthlyQuota.toLocaleString()}
            </TableCell>
            <TableCell className="hidden lg:table-cell">
              {app.transliterateToGsm7 ? "on" : "off"}
            </TableCell>
            <TableCell align="end" className="hidden md:table-cell">
              <TimestampDisplay value={app.updatedAt} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
