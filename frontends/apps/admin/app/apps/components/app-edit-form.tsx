// Dumb view: the app-edit form fields. Receives an already-configured
// `react-hook-form` instance; owns no mutation, no data fetching.

import { FormField, Input, StaleWriteBanner, Textarea } from "@vsms/ui";
import type { UseFormReturn } from "react-hook-form";
import type { AppEditValues } from "../app-forms";
import { ErrorBanner } from "./error-banner";

export function AppEditForm({
  form,
  slug,
  onSubmit,
  isStale,
  onReload,
  generalError,
}: {
  form: UseFormReturn<AppEditValues>;
  slug: string;
  onSubmit: (values: AppEditValues) => void;
  isStale: boolean;
  onReload: () => void;
  generalError: string | null;
}) {
  return (
    <form id="app-edit-form" onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <FormField label="Name" htmlFor="app-edit-name" error={form.formState.errors.name?.message}>
          <Input
            id="app-edit-name"
            aria-invalid={form.formState.errors.name != null}
            {...form.register("name")}
          />
        </FormField>
        <FormField label="Slug" htmlFor="app-edit-slug">
          <Input id="app-edit-slug" value={slug} disabled />
        </FormField>
      </div>

      <FormField label="Description" htmlFor="app-edit-description">
        <Textarea id="app-edit-description" rows={2} {...form.register("description")} />
      </FormField>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <FormField
          label="Monthly quota"
          htmlFor="app-edit-quota"
          error={form.formState.errors.monthlyQuota?.message}
        >
          <Input
            id="app-edit-quota"
            type="number"
            min="0"
            aria-invalid={form.formState.errors.monthlyQuota != null}
            {...form.register("monthlyQuota", { valueAsNumber: true })}
          />
        </FormField>
        <div className="flex items-end gap-4 pb-2">
          <label className="flex items-center gap-2 text-caption text-foreground">
            <input
              type="checkbox"
              className="checkbox checkbox-sm"
              {...form.register("transliterateToGsm7")}
            />
            Transliterate to GSM-7
          </label>
          <label className="flex items-center gap-2 text-caption text-foreground">
            <input type="checkbox" className="checkbox checkbox-sm" {...form.register("active")} />
            Active
          </label>
        </div>
      </div>

      <FormField
        label="IP allowlist (one CIDR per line — blank = unrestricted)"
        htmlFor="app-edit-allowlist"
      >
        <Textarea
          id="app-edit-allowlist"
          rows={3}
          className="font-mono text-caption"
          {...form.register("ipAllowlist")}
        />
      </FormField>

      {isStale && (
        <StaleWriteBanner
          message="Someone else changed this app since it loaded. Reload to see their edit."
          onReload={onReload}
        />
      )}
      {generalError != null && <ErrorBanner>Save failed: {generalError}</ErrorBanner>}
    </form>
  );
}
