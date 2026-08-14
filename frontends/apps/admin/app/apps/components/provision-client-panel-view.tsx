// Dumb view for the "provision a service-account client" panel. Renders
// inline, not as a `Dialog` — see `apps-screen.tsx`'s own module doc for
// the live-verified reason (a nested Headless UI `Dialog` inside an
// already-open `MoreDetailDrawer` self-dismisses the whole drawer).

import { Button, FormField, Input, Textarea, toast } from "@vsms/ui";
import type { UseFormReturn } from "react-hook-form";
import type { ProvisionClientValues } from "../app-forms";
import { Code } from "./code";
import { ErrorBanner } from "./error-banner";

export interface ProvisionedClientKey {
  clientId: string;
  privateKeyPem: string;
}

export function ProvisionClientPanelView({
  open,
  form,
  onSubmit,
  onCancel,
  onDone,
  isPending,
  isError,
  errorMessage,
  result,
}: {
  open: boolean;
  form: UseFormReturn<ProvisionClientValues>;
  onSubmit: (values: ProvisionClientValues) => void;
  onCancel: () => void;
  onDone: () => void;
  isPending: boolean;
  isError: boolean;
  errorMessage: string;
  result: ProvisionedClientKey | undefined;
}) {
  if (!open) return null;

  return (
    <div className="flex flex-col gap-4 rounded-sm border border-edge bg-surface-2 p-4">
      <div>
        <h4 className="font-medium text-body text-foreground">
          Provision a service-account client
        </h4>
        <p className="mt-1 text-caption text-muted-foreground">
          The private key is shown exactly once. It is never stored anywhere by this console or by
          sms-api — copy it now, or the client has to be retired and re-provisioned.
        </p>
      </div>

      {result === undefined && (
        <form
          id="provision-client-form"
          onSubmit={form.handleSubmit(onSubmit)}
          className="flex flex-col gap-4"
        >
          <FormField
            label="Label"
            htmlFor="client-label"
            error={form.formState.errors.label?.message}
          >
            <Input
              id="client-label"
              placeholder="e.g. billing-service"
              aria-invalid={form.formState.errors.label != null}
              {...form.register("label")}
            />
          </FormField>
          <FormField
            label="Scopes (space-separated)"
            htmlFor="client-scopes"
            error={form.formState.errors.scopes?.message}
          >
            <Input
              id="client-scopes"
              aria-invalid={form.formState.errors.scopes != null}
              {...form.register("scopes")}
            />
            {form.formState.errors.scopes == null && (
              <p className="text-caption text-subtle-foreground">
                e.g. <span className="font-mono">sms:send sms:read</span>
              </p>
            )}
          </FormField>
          {isError && <ErrorBanner>{errorMessage}</ErrorBanner>}

          <div className="flex items-center justify-end gap-2">
            <Button type="button" variant="ghost" onClick={onCancel}>
              Cancel
            </Button>
            <Button type="submit" disabled={isPending}>
              {isPending ? "Provisioning…" : "Provision"}
            </Button>
          </div>
        </form>
      )}

      {result !== undefined && (
        <div className="flex flex-col gap-3">
          <div className="rounded-sm border border-edge bg-surface-1 px-3 py-2 text-caption text-muted-foreground">
            Client id: <Code>{result.clientId}</Code>
          </div>
          <FormField
            label="Private key (PKCS#8 PEM) — save this now"
            htmlFor="provisioned-private-key"
          >
            <Textarea
              id="provisioned-private-key"
              readOnly
              rows={10}
              className="font-mono text-caption"
              value={result.privateKeyPem}
            />
            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={() => {
                  void navigator.clipboard.writeText(result.privateKeyPem);
                  toast({ title: "Private key copied", variant: "success" });
                }}
              >
                Copy key
              </Button>
            </div>
          </FormField>
          <div className="flex justify-end">
            <Button type="button" onClick={onDone}>
              I&apos;ve saved this key — close
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
