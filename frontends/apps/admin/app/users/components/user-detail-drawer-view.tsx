// Dumb view: the user detail drawer's markup. The smart `UserDetailDrawer`
// (in `users-screen.tsx`) owns the query, the form, both mutations and the
// delete-confirm boolean.

import { Button, IdDisplay, MoreDetailDrawer, Skeleton } from "@vsms/ui";
import type { ReactNode } from "react";
import type { UseFormReturn } from "react-hook-form";
import type { RoleRecord } from "../types";
import type { UserEditValues } from "../user-forms";
import { ErrorBanner } from "./error-banner";
import { UserEditForm } from "./user-edit-form";

export function UserDetailDrawerView({
  userId,
  open,
  onOpenChange,
  title,
  isLoading,
  loadError,
  hasDetail,
  form,
  roles,
  onSubmit,
  isStale,
  onReload,
  generalError,
  isSaving,
  onDeleteClick,
  onClose,
  deleteConfirm,
}: {
  userId: string | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  isLoading: boolean;
  loadError: string | null;
  hasDetail: boolean;
  form: UseFormReturn<UserEditValues>;
  roles: RoleRecord[];
  onSubmit: (values: UserEditValues) => void;
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
      description={userId !== null && <IdDisplay value={userId} variant="full" />}
      footer={
        <>
          <Button
            type="button"
            variant="destructive"
            size="sm"
            className="mr-auto"
            onClick={onDeleteClick}
          >
            Delete user
          </Button>
          <Button type="button" variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button type="submit" form="user-edit-form" disabled={isSaving || !hasDetail}>
            {isSaving ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      {isLoading && <Skeleton className="h-32 w-full" />}
      {loadError !== null && <ErrorBanner>Could not read this user: {loadError}</ErrorBanner>}

      {hasDetail && (
        <UserEditForm
          form={form}
          roles={roles}
          onSubmit={onSubmit}
          isStale={isStale}
          onReload={onReload}
          generalError={generalError}
        />
      )}

      {deleteConfirm}
    </MoreDetailDrawer>
  );
}
