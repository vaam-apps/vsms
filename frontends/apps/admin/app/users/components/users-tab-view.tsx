// Dumb view: the "Users" tab body — permission note, provision button, and
// table. The provision dialog and detail drawer are rendered by the smart
// `UsersTab` as `children`, since each owns its own mutation state.

import { Button, Code, InlineBanner } from "@vsms/ui";
import type { ReactNode } from "react";
import type { UserListItem } from "../types";
import { ErrorBanner } from "./error-banner";
import { UsersTable } from "./users-table";

export function UsersTabView({
  users,
  isLoading,
  errorMessage,
  onProvisionClick,
  onRowClick,
  children,
}: {
  users: UserListItem[];
  isLoading: boolean;
  errorMessage: string | null;
  onProvisionClick: () => void;
  onRowClick: (user: UserListItem) => void;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <InlineBanner variant="neutral">
          Provisioning and editing both require <Code>user:manage</Code> (owner, admin) — a role
          without it gets a real error here, not a silent no-op.
        </InlineBanner>
        <Button type="button" onClick={onProvisionClick}>
          Provision user
        </Button>
      </div>

      {errorMessage !== null && <ErrorBanner>Could not read users: {errorMessage}</ErrorBanner>}

      <UsersTable users={users} isLoading={isLoading} onRowClick={onRowClick} />

      {children}
    </div>
  );
}
