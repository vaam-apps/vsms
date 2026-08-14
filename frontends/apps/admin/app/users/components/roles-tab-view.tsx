// Dumb view: the "Roles" tab body — permission note, "New role" button,
// and table.

import { Button } from "@vsms/ui";
import type { ReactNode } from "react";
import type { RoleRecord } from "../types";
import { ErrorBanner } from "./error-banner";
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
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
          Creating, editing, and deleting roles all require{" "}
          <span className="font-mono text-foreground">owner</span> — the narrowest write action in
          this console.
        </div>
        <Button type="button" onClick={onCreateClick}>
          New role
        </Button>
      </div>

      {errorMessage !== null && <ErrorBanner>Could not read roles: {errorMessage}</ErrorBanner>}

      <RolesTable roles={roles} isLoading={isLoading} onRowClick={onRowClick} />

      {children}
    </div>
  );
}
