// Dumb view: the role-edit form fields.

import { FormField, Input, StaleWriteBanner, Textarea } from "@vsms/ui";
import type { UseFormReturn } from "react-hook-form";
import { KNOWN_PERMISSIONS, type RoleEditValues } from "../role-forms";
import { ErrorBanner } from "./error-banner";

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
      <FormField
        label="Label"
        htmlFor="role-edit-label"
        error={form.formState.errors.label?.message}
      >
        <Input
          id="role-edit-label"
          aria-invalid={form.formState.errors.label != null}
          {...form.register("label")}
        />
      </FormField>
      <FormField label="Permissions (space-separated)" htmlFor="role-edit-permissions">
        <Textarea
          id="role-edit-permissions"
          rows={3}
          className="font-mono text-caption"
          {...form.register("permissions")}
        />
        <p className="text-caption text-subtle-foreground">
          Known literals: {KNOWN_PERMISSIONS.join(", ")}
        </p>
      </FormField>

      {isStale && <StaleWriteBanner onReload={onReload} />}
      {generalError != null && <ErrorBanner>Save failed: {generalError}</ErrorBanner>}
    </form>
  );
}
