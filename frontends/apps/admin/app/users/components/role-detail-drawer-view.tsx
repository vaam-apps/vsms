// Dumb view: the role detail drawer's markup. The smart `RoleDetailDrawer`
// (in `users-screen.tsx`) owns the query, the form, both mutations and the
// delete-confirm boolean.

import { Button, MoreDetailDrawer, Skeleton } from "@vsms/ui";
import type { ReactNode } from "react";
import type { UseFormReturn } from "react-hook-form";
import type { RoleEditValues } from "../role-forms";
import { ErrorBanner } from "./error-banner";
import { RoleEditForm } from "./role-edit-form";

export function RoleDetailDrawerView({
  open,
  onOpenChange,
  title,
  roleKey,
  isLoading,
  loadError,
  hasDetail,
  builtin,
  form,
  onSubmit,
  isStale,
  onReload,
  generalError,
  isSaving,
  onDeleteClick,
  onClose,
  deleteConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  roleKey: string | null;
  isLoading: boolean;
  loadError: string | null;
  hasDetail: boolean;
  builtin: boolean;
  form: UseFormReturn<RoleEditValues>;
  onSubmit: (values: RoleEditValues) => void;
  isStale: boolean;
  onReload: () => void;
  generalError: string | null;
  isSaving: boolean;
  onDeleteClick: () => void;
  onClose: () => void;
  deleteConfirm: ReactNode;
}) {
  return (
    <MoreDetailDrawer
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      description={roleKey !== null && <span className="font-mono">{roleKey}</span>}
      footer={
        <>
          {!builtin && (
            <Button
              type="button"
              variant="destructive"
              size="sm"
              className="mr-auto"
              onClick={onDeleteClick}
            >
              Delete role
            </Button>
          )}
          {builtin && (
            <span className="mr-auto self-center text-caption text-subtle-foreground">
              Built-in role — cannot be deleted.
            </span>
          )}
          <Button type="button" variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button type="submit" form="role-edit-form" disabled={isSaving || !hasDetail}>
            {isSaving ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      {isLoading && <Skeleton className="h-32 w-full" />}
      {loadError !== null && <ErrorBanner>Could not read this role: {loadError}</ErrorBanner>}

      {hasDetail && (
        <RoleEditForm
          form={form}
          onSubmit={onSubmit}
          isStale={isStale}
          onReload={onReload}
          generalError={generalError}
        />
      )}

      {deleteConfirm !== null && <div className="mt-4">{deleteConfirm}</div>}
    </MoreDetailDrawer>
  );
}
