// Dumb view: the service-account clients table nested inside an app's
// detail drawer.

import {
  Button,
  IdDisplay,
  InlineEmptyState,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@vsms/ui";
import type { AppClientListItem } from "../types";
import { ErrorBanner } from "./error-banner";

export function AppClientsTable({
  clients,
  isLoading,
  errorMessage,
  onRetireClick,
}: {
  clients: AppClientListItem[];
  isLoading: boolean;
  errorMessage: string | null;
  onRetireClick: (client: AppClientListItem) => void;
}) {
  return (
    <>
      {errorMessage !== null && <ErrorBanner>{errorMessage}</ErrorBanner>}

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Label</TableHead>
            <TableHead className="hidden sm:table-cell">Client id</TableHead>
            <TableHead className="hidden md:table-cell">Scopes</TableHead>
            <TableHead>Active</TableHead>
            <TableHead align="end">Actions</TableHead>
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
          {!isLoading && clients.length === 0 && (
            <TableRow>
              <TableCell colSpan={5}>
                <InlineEmptyState message="No clients provisioned for this app yet." />
              </TableCell>
            </TableRow>
          )}
          {clients.map((client) => (
            <TableRow key={client.id}>
              <TableCell>{client.label}</TableCell>
              <TableCell mono className="hidden sm:table-cell">
                <IdDisplay value={client.clientId} />
              </TableCell>
              <TableCell mono className="hidden text-caption md:table-cell">
                {client.scopes.trim()}
              </TableCell>
              <TableCell>
                {client.active ? (
                  <span className="text-state-success-fg">active</span>
                ) : (
                  <span className="text-muted-foreground">retired</span>
                )}
              </TableCell>
              <TableCell align="end">
                {client.active && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => onRetireClick(client)}
                  >
                    Retire
                  </Button>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </>
  );
}
