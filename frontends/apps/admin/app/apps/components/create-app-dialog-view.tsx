// Dumb view: the "New app" dialog markup. Receives an already-configured
// `react-hook-form` instance and a submit handler; owns no mutation, no
// validation rule, no data fetching of its own.

import {
  Button,
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  FormField,
  Input,
} from "@vsms/ui";
import type { UseFormReturn } from "react-hook-form";
import type { AppCreateValues } from "../app-forms";
import { ErrorBanner } from "./error-banner";

export function CreateAppDialogView({
  open,
  onOpenChange,
  form,
  onSubmit,
  isPending,
  generalError,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  form: UseFormReturn<AppCreateValues>;
  onSubmit: (values: AppCreateValues) => void;
  isPending: boolean;
  generalError: string | null;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[480px]">
        <DialogHeader>
          <DialogTitle>New app</DialogTitle>
        </DialogHeader>
        <form
          id="create-app-form"
          onSubmit={form.handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <FormField label="Name" htmlFor="app-name" error={form.formState.errors.name?.message}>
            <Input
              id="app-name"
              aria-invalid={form.formState.errors.name != null}
              {...form.register("name")}
            />
          </FormField>
          <FormField label="Slug" htmlFor="app-slug" error={form.formState.errors.slug?.message}>
            <Input
              id="app-slug"
              placeholder="lowercase-with-hyphens"
              aria-invalid={form.formState.errors.slug != null}
              {...form.register("slug")}
            />
          </FormField>
          <FormField
            label="Monthly quota"
            htmlFor="app-quota"
            error={form.formState.errors.monthlyQuota?.message}
          >
            <Input
              id="app-quota"
              type="number"
              min="0"
              aria-invalid={form.formState.errors.monthlyQuota != null}
              {...form.register("monthlyQuota", { valueAsNumber: true })}
            />
          </FormField>
          {generalError != null && <ErrorBanner>{generalError}</ErrorBanner>}
        </form>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" form="create-app-form" disabled={isPending}>
            {isPending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
