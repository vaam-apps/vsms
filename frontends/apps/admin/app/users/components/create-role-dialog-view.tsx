// Dumb view: the "New role" dialog markup.

import {
  Button,
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  Textarea,
} from "@vsms/ui";
import type { UseFormReturn } from "react-hook-form";
import { KNOWN_PERMISSIONS, type RoleCreateValues } from "../role-forms";
import { ErrorBanner } from "./error-banner";

export function CreateRoleDialogView({
  open,
  onOpenChange,
  form,
  onSubmit,
  isPending,
  generalError,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  form: UseFormReturn<RoleCreateValues>;
  onSubmit: (values: RoleCreateValues) => void;
  isPending: boolean;
  generalError: string | null;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[520px]">
        <DialogHeader>
          <DialogTitle>New role</DialogTitle>
        </DialogHeader>
        <form
          id="create-role-form"
          onSubmit={form.handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="role-key">Key</Label>
            <Input
              id="role-key"
              placeholder="lowercase_with_underscores"
              aria-invalid={form.formState.errors.key != null}
              {...form.register("key")}
            />
            {form.formState.errors.key != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.key.message}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="role-label">Label</Label>
            <Input
              id="role-label"
              aria-invalid={form.formState.errors.label != null}
              {...form.register("label")}
            />
            {form.formState.errors.label != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.label.message}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="role-permissions">Permissions (space-separated)</Label>
            <Textarea
              id="role-permissions"
              rows={3}
              className="font-mono text-caption"
              {...form.register("permissions")}
            />
            <p className="text-caption text-subtle-foreground">
              Known literals: {KNOWN_PERMISSIONS.join(", ")}
            </p>
          </div>
          {generalError != null && <ErrorBanner>{generalError}</ErrorBanner>}
        </form>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" form="create-role-form" disabled={isPending}>
            {isPending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
