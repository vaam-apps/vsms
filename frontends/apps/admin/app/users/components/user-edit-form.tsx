// Dumb view: the user-edit form fields.

import {
  FormField,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  StaleWriteBanner,
} from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import type { RoleRecord } from "../types";
import type { UserEditValues } from "../user-forms";
import { ErrorBanner } from "./error-banner";

export function UserEditForm({
  form,
  roles,
  onSubmit,
  isStale,
  onReload,
  generalError,
}: {
  form: UseFormReturn<UserEditValues>;
  roles: RoleRecord[];
  onSubmit: (values: UserEditValues) => void;
  isStale: boolean;
  onReload: () => void;
  generalError: string | null;
}) {
  return (
    <form
      id="user-edit-form"
      onSubmit={form.handleSubmit(onSubmit)}
      className="flex flex-col gap-4"
    >
      <FormField
        label="Display name"
        htmlFor="user-edit-name"
        error={form.formState.errors.displayName?.message}
      >
        <Input
          id="user-edit-name"
          aria-invalid={form.formState.errors.displayName != null}
          {...form.register("displayName")}
        />
      </FormField>
      <FormField label="Role" htmlFor="user-edit-role">
        <Controller
          control={form.control}
          name="roleKey"
          render={({ field }) => (
            <Select value={field.value} onValueChange={field.onChange}>
              <SelectTrigger id="user-edit-role">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {roles.map((role) => (
                  <SelectItem key={role.key} value={role.key}>
                    {role.label} ({role.key})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </FormField>
      <label className="flex items-center gap-2 text-caption text-foreground">
        <input type="checkbox" className="checkbox checkbox-sm" {...form.register("active")} />
        Active
      </label>

      {isStale && <StaleWriteBanner onReload={onReload} />}
      {generalError != null && <ErrorBanner>Save failed: {generalError}</ErrorBanner>}
    </form>
  );
}
