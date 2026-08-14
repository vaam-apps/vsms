// Dumb view: the app detail drawer's markup. The smart `AppDetailDrawer`
// (in `apps-screen.tsx`) owns the query, the form, both mutations and the
// delete-confirm boolean; this component only lays it out.

import { Button, IdDisplay, MoreDetailDrawer, Skeleton } from "@vsms/ui";
import type { ReactNode } from "react";
import type { UseFormReturn } from "react-hook-form";
import type { AppEditValues } from "../app-forms";
import { AppEditForm } from "./app-edit-form";
import { ErrorBanner } from "./error-banner";

export function AppDetailDrawerView({
  appId,
  open,
  onOpenChange,
  title,
  isLoading,
  loadError,
  hasDetail,
  form,
  slug,
  onSubmit,
  isStale,
  onReload,
  generalError,
  isSaving,
  onDeleteClick,
  onClose,
  deleteConfirm,
  clientsPanel,
}: {
  appId: string | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  isLoading: boolean;
  loadError: string | null;
  hasDetail: boolean;
  form: UseFormReturn<AppEditValues>;
  slug: string;
  onSubmit: (values: AppEditValues) => void;
  isStale: boolean;
  onReload: () => void;
  generalError: string | null;
  isSaving: boolean;
  onDeleteClick: () => void;
  onClose: () => void;
  /** Rendered inline rather than as a nested `Dialog` — see this route's
   * own module doc. `null` when no delete is in progress. */
  deleteConfirm: ReactNode;
  clientsPanel: ReactNode;
}) {
  return (
    <MoreDetailDrawer
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      description={appId !== null && <IdDisplay value={appId} variant="full" />}
      footer={
        <>
          <Button
            type="button"
            variant="destructive"
            size="sm"
            className="mr-auto"
            onClick={onDeleteClick}
          >
            Delete app
          </Button>
          <Button type="button" variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button type="submit" form="app-edit-form" disabled={isSaving || !hasDetail}>
            {isSaving ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      {isLoading && <Skeleton className="h-32 w-full" />}
      {loadError !== null && <ErrorBanner>Could not read this app: {loadError}</ErrorBanner>}

      {appId !== null && hasDetail && (
        <div className="flex flex-col gap-6">
          <AppEditForm
            form={form}
            slug={slug}
            onSubmit={onSubmit}
            isStale={isStale}
            onReload={onReload}
            generalError={generalError}
          />

          {deleteConfirm}

          <div className="border-edge border-t pt-4">{clientsPanel}</div>
        </div>
      )}
    </MoreDetailDrawer>
  );
}
