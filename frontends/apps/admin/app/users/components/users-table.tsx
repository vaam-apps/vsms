// Dumb view: the users list table.

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
import type { UserListItem } from "../types";

export function UsersTable({
  users,
  isLoading,
  onRowClick,
}: {
  users: UserListItem[];
  isLoading: boolean;
  onRowClick: (user: UserListItem) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Active</TableHead>
          <TableHead>Email</TableHead>
          <TableHead className="hidden sm:table-cell">Display name</TableHead>
          <TableHead>Role</TableHead>
          <TableHead align="end" className="hidden md:table-cell">
            Last login
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {isLoading && (
          <TableRow>
            <TableCell colSpan={5}>
              <Skeleton className="h-4 w-full" />
            </TableCell>
          </TableRow>
        )}
        {!isLoading && users.length === 0 && (
          <TableRow>
            <TableCell colSpan={5}>
              <InlineEmptyState message="No users provisioned yet." />
            </TableCell>
          </TableRow>
        )}
        {users.map((user) => (
          <TableRow key={user.id} className="cursor-pointer" onClick={() => onRowClick(user)}>
            <TableCell>
              {user.active ? (
                <span className="text-state-success-fg">yes</span>
              ) : (
                <span className="text-muted-foreground">no</span>
              )}
            </TableCell>
            <TableCell>{user.email}</TableCell>
            <TableCell className="hidden sm:table-cell">{user.displayName}</TableCell>
            <TableCell mono>{user.roleKey}</TableCell>
            <TableCell align="end" className="hidden md:table-cell">
              {user.lastLoginAt !== undefined ? (
                <TimestampDisplay value={user.lastLoginAt} />
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
