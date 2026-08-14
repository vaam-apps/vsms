import {
  InlineConfirm,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vsms/ui";
import type { ProviderListItem } from "../sender-id-domain";

// Dumb (R6): "register with a provider" — a short *form*, not a yes/no
// confirm, which is exactly why `InlineConfirm`'s `children` slot exists
// (see that component's own doc comment). Rendered *inline* inside the
// sender id's `MoreDetailDrawer` body — never a nested `Dialog`. See
// `frontends/apps/admin/app/gallery/page.tsx`'s
// `NestedDialogInDrawerRegression` for why.
export function RegisterProviderConfirm({
  senderIdValue,
  unregisteredProviders,
  selectedProviderId,
  onSelectProvider,
  pending,
  errorMessage,
  onConfirm,
  onCancel,
}: {
  senderIdValue: string;
  unregisteredProviders: ProviderListItem[];
  selectedProviderId: string | null;
  onSelectProvider: (providerId: string) => void;
  pending: boolean;
  errorMessage?: string | undefined;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <InlineConfirm
      title={`Register ${senderIdValue} with a provider`}
      description="Creates a new registration row for this (sender ID, provider) pair, status pending, submitted now."
      confirmLabel="Register"
      pendingLabel="Registering…"
      pending={pending}
      confirmDisabled={selectedProviderId === null}
      destructive={false}
      error={errorMessage != null ? `Failed: ${errorMessage}` : undefined}
      onConfirm={onConfirm}
      onCancel={onCancel}
    >
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="register-provider">Provider</Label>
        <Select
          {...(selectedProviderId !== null ? { value: selectedProviderId } : {})}
          onValueChange={onSelectProvider}
        >
          <SelectTrigger id="register-provider">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {unregisteredProviders.map((provider) => (
              <SelectItem key={provider.id} value={provider.id}>
                {provider.displayName} ({provider.key})
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </InlineConfirm>
  );
}
