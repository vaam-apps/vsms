// Dumb view: the role-edit form fields.

import { Input, Label, Textarea } from "@vsms/ui";
import type { UseFormReturn } from "react-hook-form";
import { KNOWN_PERMISSIONS, type RoleEditValues } from "../role-forms";
import { ErrorBanner } from "./error-banner";
import { StaleWriteBanner } from "./stale-write-banner";

export function RoleEditForm({
  form,
  onSubmit,
  isStale,
  onReload,
  generalError,
}: {
  form: UseFormReturn<RoleEditValues>;
  onSubmit: (values: RoleEditValues) => void;
  isStale: boolean;
  onReload: () => void;
  generalError: string | null;
}) {
  return (
    <form
      id="role-edit-form"
      onSubmit={form.handleSubmit(onSubmit)}
      className="flex flex-col gap-4"
    >
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="role-edit-label">Label</Label>
        <Input
          id="role-edit-label"
          aria-invalid={form.formState.errors.label != null}
          {...form.register("label")}
        />
        {form.formState.errors.label != null && (
          <p className="text-caption text-state-danger-fg">{form.formState.errors.label.message}</p>
        )}
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="role-edit-permissions">Permissions (space-separated)</Label>
        <Textarea
          id="role-edit-permissions"
          rows={3}
          className="font-mono text-caption"
          {...form.register("permissions")}
        />
        <p className="text-caption text-subtle-foreground">
          Known literals: {KNOWN_PERMISSIONS.join(", ")}
        </p>
      </div>

      {isStale && <StaleWriteBanner onReload={onReload} />}
      {generalError != null && <ErrorBanner>Save failed: {generalError}</ErrorBanner>}
    </form>
  );
}
