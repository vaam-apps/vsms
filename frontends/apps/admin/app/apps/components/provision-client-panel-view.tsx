// Dumb view for the "provision a service-account client" panel. Renders
// inline, not as a `Dialog` — see `apps-screen.tsx`'s own module doc for
// the live-verified reason (a nested Headless UI `Dialog` inside an
// already-open `MoreDetailDrawer` self-dismisses the whole drawer).

import { Button, ChipSelect, Code, FormField, groupLabelId, Input, Textarea } from "@vaam-apps/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import { ErrorBanner } from "@/components/error-banner";
import type { ProvisionClientValues } from "../app-forms";
import { KNOWN_SCOPES, parseScopes, SCOPE_DESCRIPTIONS, serializeScopes } from "../scopes";

export interface ProvisionedClientKey {
  clientId: string;
  thumbprint: string;
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
          The client signs every request with an Ed25519 key you generate. Only the public half is
          sent here; the private half never reaches the gateway, so there is nothing to copy back.
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
          {/* Chips over the real vocabulary, not a space-separated text box.
              Before this an operator had to already know both that scopes
              are space-delimited *and* what the fourteen valid strings
              are, with a typo silently producing a client that is denied
              at Layer 2 with no hint why. See `../scopes.ts` for how that
              list was derived from what the server actually enforces. */}
          <FormField
            label="Scopes"
            htmlFor="client-scopes"
            control="group"
            error={form.formState.errors.scopes?.message}
          >
            <Controller
              control={form.control}
              name="scopes"
              render={({ field }) => (
                <ChipSelect
                  aria-labelledby={groupLabelId("client-scopes")}
                  value={parseScopes(field.value)}
                  onValueChange={(next) => field.onChange(serializeScopes(next))}
                  options={KNOWN_SCOPES.map((scope) => ({
                    value: scope,
                    label: scope,
                    description: SCOPE_DESCRIPTIONS[scope],
                  }))}
                />
              )}
            />
          </FormField>
          <FormField
            label="Public key (Ed25519, base64url)"
            htmlFor="client-public-key"
            error={form.formState.errors.publicKey?.message}
          >
            <Textarea
              id="client-public-key"
              rows={2}
              className="font-mono text-caption"
              placeholder="43 characters, e.g. 11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo"
              aria-invalid={form.formState.errors.publicKey != null}
              {...form.register("publicKey")}
            />
            <p className="text-caption text-subtle-foreground">
              Make the key with <Code>openssl genpkey -algorithm ed25519 -out key.pem</Code>, then
              print its public half with{" "}
              <Code>
                openssl pkey -in key.pem -pubout -outform DER | tail -c 32 | base64 | tr
                &apos;+/&apos; &apos;-_&apos; | tr -d &apos;=\n&apos;
              </Code>
              .
            </p>
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
          <div className="rounded-sm border border-edge bg-surface-1 px-3 py-2 text-caption text-muted-foreground">
            Key thumbprint: <Code>{result.thumbprint}</Code>
          </div>
          <div className="flex justify-end">
            <Button type="button" onClick={onDone}>
              Close
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
