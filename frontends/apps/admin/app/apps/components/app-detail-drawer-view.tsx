// Dumb view: the app detail drawer's markup. The smart `AppDetailDrawer`
// (in `apps-screen.tsx`) owns the query, the form, both mutations and the
// delete-confirm boolean; this component only lays it out.

import { Button, IdDisplay, MoreDetailDrawer, SkeletonText } from "@vaam-apps/ui";
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
      {/* SkeletonText, not a flat slab: what's arriving is a multi-field
          edit form (`AppEditForm`), and a paragraph-shaped placeholder
          with a short last line reads as prose/form fields rather than a
          single indistinct block. `lines={6}` approximates the same
          visual weight the previous `h-32` slab held. */}
      {isLoading && <SkeletonText lines={6} />}
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
