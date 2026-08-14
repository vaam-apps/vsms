import { Badge } from "@vsms/ui";

// `SenderIdRegistration.status` is a bare `String`, not part of the
// governed `StatusPill` vocabulary (`schema.cstack` never closed it into an
// enum, and only `"approved"` is load-bearing server-side) — rendered as a
// `Badge` with a colour hint layered on top locally, never a fake
// `StatusPill` for a value the schema itself never closed into an enum. See
// sender-ids-screen.tsx's own module doc for the full reasoning.
const STATUS_CLASSES: Record<string, string> = {
  approved: "text-state-success-fg border-state-success-border bg-state-success-bg",
  rejected: "text-state-danger-fg border-state-danger-border bg-state-danger-bg",
  pending: "text-muted-foreground",
  submitted: "text-muted-foreground",
};

export function RegistrationStatusBadge({ status }: { status: string }) {
  const extra = STATUS_CLASSES[status] ?? "text-muted-foreground";
  return (
    <Badge variant={status in STATUS_CLASSES ? "neutral" : "outline"} className={extra}>
      {status}
    </Badge>
  );
}
