// Dumb view: the "Users" tab body — permission note, provision button, and
// table. The provision dialog and detail drawer are rendered by the smart
// `UsersTab` as `children`, since each owns its own mutation state.

import { Button } from "@vaam-apps/ui";
import type { ReactNode } from "react";
import { ErrorBanner } from "@/components/error-banner";
import { ListSurface } from "@/components/list-surface";
import { ListToolbarRow } from "@/components/list-toolbar-row";
import type { UserListItem } from "../types";
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
      {errorMessage !== null && <ErrorBanner>Could not read users: {errorMessage}</ErrorBanner>}

      <ListSurface
        toolbar={
          <ListToolbarRow
            note="Provisioning and editing users need the owner or admin role."
            action={
              <Button type="button" onClick={onProvisionClick}>
                Provision user
              </Button>
            }
          />
        }
      >
        <UsersTable users={users} isLoading={isLoading} onRowClick={onRowClick} />
      </ListSurface>

      {children}
    </div>
  );
}
