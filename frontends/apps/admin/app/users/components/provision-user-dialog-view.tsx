// Dumb view: the "provision a console account" dialog. Shows the returned
// one-time password exactly once — see `apps-screen.tsx`'s own module doc
// for the identical discipline applied there to a private key.

import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  toast,
} from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import type { RoleRecord } from "../types";
import type { ProvisionUserValues } from "../user-forms";
import { ErrorBanner } from "./error-banner";

export interface ProvisionedUser {
  email: string;
  roleKey: string;
  password: string;
}

export function ProvisionUserDialogView({
  open,
  roles,
  form,
  onSubmit,
  isPending,
  isError,
  errorMessage,
  result,
  onDone,
}: {
  open: boolean;
  roles: RoleRecord[];
  form: UseFormReturn<ProvisionUserValues>;
  onSubmit: (values: ProvisionUserValues) => void;
  isPending: boolean;
  isError: boolean;
  errorMessage: string;
  result: ProvisionedUser | undefined;
  onDone: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={(next) => (next ? undefined : onDone())}>
      <DialogContent className="max-w-[480px]">
        <DialogHeader>
          <DialogTitle>Provision a console account</DialogTitle>
          <DialogDescription>
            The one-time password is shown exactly once — copy it now, or the account has to be
            deactivated and provisioned again under a different email.
          </DialogDescription>
        </DialogHeader>

        {result === undefined && (
          <form
            id="provision-user-form"
            onSubmit={form.handleSubmit(onSubmit)}
            className="flex flex-col gap-4"
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-email">Email</Label>
              <Input
                id="user-email"
                type="email"
                aria-invalid={form.formState.errors.email != null}
                {...form.register("email")}
              />
              {form.formState.errors.email != null && (
                <p className="text-caption text-state-danger-fg">
                  {form.formState.errors.email.message}
                </p>
              )}
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-display-name">Display name</Label>
              <Input
                id="user-display-name"
                aria-invalid={form.formState.errors.displayName != null}
                {...form.register("displayName")}
              />
              {form.formState.errors.displayName != null && (
                <p className="text-caption text-state-danger-fg">
                  {form.formState.errors.displayName.message}
                </p>
              )}
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="user-role">Role</Label>
              <Controller
                control={form.control}
                name="roleKey"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger id="user-role">
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
            </div>
            {isError && <ErrorBanner>{errorMessage}</ErrorBanner>}
          </form>
        )}

        {result !== undefined && (
          <div className="flex flex-col gap-3">
            <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
              {result.email} — role{" "}
              <span className="font-mono text-foreground">{result.roleKey}</span>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>One-time password — save this now</Label>
              <div className="flex items-center gap-2">
                <Input readOnly className="font-mono" value={result.password} />
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    void navigator.clipboard.writeText(result.password);
                    toast({ title: "Password copied", variant: "success" });
                  }}
                >
                  Copy
                </Button>
              </div>
              <p className="text-caption text-subtle-foreground">
                Share this over a channel the recipient controls, not this screen&apos;s own log.
              </p>
            </div>
          </div>
        )}

        <DialogFooter>
          {result === undefined ? (
            <>
              <Button type="button" variant="ghost" onClick={onDone}>
                Cancel
              </Button>
              <Button type="submit" form="provision-user-form" disabled={isPending}>
                {isPending ? "Provisioning…" : "Provision"}
              </Button>
            </>
          ) : (
            <Button type="button" onClick={onDone}>
              I&apos;ve saved this password — close
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
