// Dumb view: the "Roles" tab body — permission note, "New role" button,
// and table.

import { Button, Code, InlineBanner } from "@vsms/ui";
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
        <InlineBanner variant="neutral">
          Creating, editing, and deleting roles all require <Code>owner</Code> — the narrowest write
          action in this console.
        </InlineBanner>
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
