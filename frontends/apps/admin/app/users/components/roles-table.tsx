// Dumb view: the roles list table.

import {
  Badge,
  InlineEmptyState,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@vsms/ui";
import type { RoleRecord } from "../types";

export function RolesTable({
  roles,
  isLoading,
  onRowClick,
}: {
  roles: RoleRecord[];
  isLoading: boolean;
  onRowClick: (role: RoleRecord) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Key</TableHead>
          <TableHead>Label</TableHead>
          <TableHead hideBelow="sm">Built-in</TableHead>
          <TableHead hideBelow="md">Permissions</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {isLoading && (
          <TableRow>
            <TableCell colSpan={4}>
              <Skeleton className="h-4 w-full" />
            </TableCell>
          </TableRow>
        )}
        {!isLoading && roles.length === 0 && (
          <TableRow>
            <TableCell colSpan={4}>
              <InlineEmptyState message="No roles yet." />
            </TableCell>
          </TableRow>
        )}
        {roles.map((role) => (
          <TableRow key={role.id} className="cursor-pointer" onClick={() => onRowClick(role)}>
            <TableCell mono>{role.key}</TableCell>
            <TableCell>{role.label}</TableCell>
            <TableCell hideBelow="sm">
              {role.builtin ? <Badge variant="outline">built-in</Badge> : "no"}
            </TableCell>
            <TableCell hideBelow="md" className="max-w-[420px] truncate text-caption">
              {role.permissions.trim()}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
