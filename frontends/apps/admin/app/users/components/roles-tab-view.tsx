// Dumb view: the "Roles" tab body — permission note, "New role" button,
// and table.

import { Button } from "@vaam-apps/ui";
import type { ReactNode } from "react";
import { ErrorBanner } from "@/components/error-banner";
import { ListSurface } from "@/components/list-surface";
import { ListToolbarRow } from "@/components/list-toolbar-row";
import type { RoleRecord } from "../types";
import { RolesTable } from "./roles-table";

export function RolesTabView({
  roles,
  isLoading,
  errorMessage,
  onCreateClick,
  onRowClick,
  children,
}: {
  roles: RoleRecord[];
  isLoading: boolean;
  errorMessage: string | null;
  onCreateClick: () => void;
  onRowClick: (role: RoleRecord) => void;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-4">
      {errorMessage !== null && <ErrorBanner>Could not read roles: {errorMessage}</ErrorBanner>}

      <ListSurface
        toolbar={
          <ListToolbarRow
            note="Creating, editing and deleting roles need the owner role."
            action={
              <Button type="button" onClick={onCreateClick}>
                New role
              </Button>
            }
          />
        }
      >
        <RolesTable roles={roles} isLoading={isLoading} onRowClick={onRowClick} />
      </ListSurface>

      {children}
    </div>
  );
}
