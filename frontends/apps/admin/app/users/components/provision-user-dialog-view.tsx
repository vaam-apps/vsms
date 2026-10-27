// Dumb view: the "provision a console account" dialog. It makes the account
// and nothing to sign in with — there is no password (VS-4); the person pairs
// a browser key, which the confirmation below explains.

import {
  Button,
  Code,
  Dialog,
  DialogActions,
  DialogClose,
  DialogDescription,
  DialogFullScreen,
  DialogHeader,
  DialogTitle,
  FormField,
  InlineBanner,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vaam-apps/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import { ErrorBanner } from "@/components/error-banner";
import type { RoleRecord } from "../types";
import type { ProvisionUserValues } from "../user-forms";

export interface ProvisionedUser {
  email: string;
  roleKey: string;
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
      <DialogFullScreen>
        <DialogHeader>
          <DialogTitle>Provision a console account</DialogTitle>
          <DialogDescription>
            Creates the account only. It has no password: the person signs in by pairing their
            browser, approved on a vaam admin phone or enrolled on the command line.
          </DialogDescription>
        </DialogHeader>

        {result === undefined && (
          <form
            id="provision-user-form"
            onSubmit={form.handleSubmit(onSubmit)}
            className="flex flex-col gap-4"
          >
            <FormField
              label="Email"
              htmlFor="user-email"
              error={form.formState.errors.email?.message}
            >
              <Input
                id="user-email"
                type="email"
                aria-invalid={form.formState.errors.email != null}
                {...form.register("email")}
              />
            </FormField>
            <FormField
              label="Display name"
              htmlFor="user-display-name"
              error={form.formState.errors.displayName?.message}
            >
              <Input
                id="user-display-name"
                aria-invalid={form.formState.errors.displayName != null}
                {...form.register("displayName")}
              />
            </FormField>
            <FormField label="Role" htmlFor="user-role">
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
            </FormField>
            {isError && <ErrorBanner>{errorMessage}</ErrorBanner>}
          </form>
        )}

        {result !== undefined && (
          <div className="flex flex-col gap-3">
            <InlineBanner variant="neutral">
              {result.email} — role <Code>{result.roleKey}</Code>
            </InlineBanner>
            <p className="text-caption text-subtle-foreground">
              Next, the person opens this console on their own device, which shows a pairing code
              for a vaam admin phone to approve. Without a phone, an operator pairs their key on the
              command line:
            </p>
            <Code>{`sms-gateway pair-console-key --user ${result.email} --public-key <the k of the pairing code>`}</Code>
          </div>
        )}

        <DialogActions>
          {result === undefined ? (
            <>
              <DialogClose as={Button} variant="ghost">
                Cancel
              </DialogClose>
              <Button type="submit" form="provision-user-form" disabled={isPending}>
                {isPending ? "Provisioning…" : "Provision"}
              </Button>
            </>
          ) : (
            <Button type="button" onClick={onDone}>
              Close
            </Button>
          )}
        </DialogActions>
      </DialogFullScreen>
    </Dialog>
  );
}
