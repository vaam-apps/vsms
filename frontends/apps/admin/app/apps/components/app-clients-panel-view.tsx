// Dumb view: the "Service-account clients" section header + table
// composition nested inside an app's detail drawer. The retire
// confirmation and provision panel are composed by the smart
// `AppClientsPanel` around this — passed in as `children` slots rather
// than known about here.

import { Button } from "@vsms/ui";
import type { ReactNode } from "react";
import type { AppClientListItem } from "../types";
import { AppClientsTable } from "./app-clients-table";

export function AppClientsPanelView({
  clients,
  isLoading,
  errorMessage,
  onProvisionClick,
  onRetireClick,
  children,
}: {
  clients: AppClientListItem[];
  isLoading: boolean;
  errorMessage: string | null;
  onProvisionClick: () => void;
  onRetireClick: (client: AppClientListItem) => void;
  /** The retire-confirm panel and the provision panel, rendered by the
   * smart layer — both need their own mutation state, so this view only
   * reserves the slot for them. */
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="font-medium text-body text-foreground">Service-account clients</h3>
        <Button type="button" size="sm" onClick={onProvisionClick}>
          Provision client
        </Button>
      </div>

      <AppClientsTable
        clients={clients}
        isLoading={isLoading}
        errorMessage={errorMessage}
        onRetireClick={onRetireClick}
      />

      {children}
    </div>
  );
}
