import { InlineConfirm } from "@vsms/ui";
import type { AttemptListItem } from "../webhook-domain";

// Dumb (R6): the replay confirmation, rendered *inline* inside the
// attempt's own `QuickDetailDrawer` body — never a nested `Dialog`. See
// `frontends/apps/admin/app/gallery/page.tsx`'s
// `NestedDialogInDrawerRegression`: the same broken-focus-trap mechanism
// affects a `QuickDetailDrawer` exactly as much as a `MoreDetailDrawer`,
// since `vaul` never forwards its own `modal` prop down to
// `@radix-ui/react-dialog`'s `Root` — `dimmed={false}` only changes the
// overlay, not the (unconditional) focus trap.
export function AttemptReplayConfirm({
  attempt,
  endpointUrl,
  pending,
  errorMessage,
  onConfirm,
  onCancel,
}: {
  attempt: AttemptListItem;
  endpointUrl: string;
  pending: boolean;
  errorMessage?: string | undefined;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <InlineConfirm
      title="Replay this delivery?"
      description={
        <>
          Re-fires exactly one attempt of{" "}
          <span className="font-mono text-foreground">{attempt.eventType}</span> to{" "}
          <span className="font-mono text-foreground">{endpointUrl}</span>, using the payload
          captured when the event first fired — not a fresh copy of the message's current state, and
          possibly old. Also clears the endpoint's circuit breaker if it was open.
        </>
      }
      confirmLabel="Replay"
      pendingLabel="Replaying…"
      destructive={false}
      pending={pending}
      error={errorMessage != null ? `Replay failed: ${errorMessage}` : undefined}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}
