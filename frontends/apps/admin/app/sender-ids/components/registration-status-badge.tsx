import { Badge, cn, isQuietHue } from "@vaam-apps/ui";

// `SenderIdRegistration.status` is a bare `String`, not part of the
// governed `StatusPill` vocabulary (`schema.cstack` never closed it into an
// enum, and only `"approved"` is load-bearing server-side) — rendered as a
// `Badge` with a colour hint layered on top locally, never a fake
// `StatusPill` for a value the schema itself never closed into an enum. See
// sender-ids-screen.tsx's own module doc for the full reasoning.
const STATUS_CLASSES: Record<string, string> = {
  // `success` is a quiet hue (`isQuietHue`) — its `--state-success-border`/
  // `-bg` tokens are `transparent` by design, and an unlayered Tailwind
  // utility outranks daisyUI's own layered `badge-neutral` background
  // (see the library's `pitfalls.md`), so painting them unconditionally
  // knocked the `Badge`'s own chrome out entirely rather than tinting it.
  // Draw the achromatic surface every quiet hue uses instead, and keep
  // only the hue's own foreground colour.
  approved: isQuietHue("success")
    ? cn("text-state-success-fg", "border-edge", "bg-surface-2")
    : cn("text-state-success-fg", "border-state-success-border", "bg-state-success-bg"),
  rejected: cn("text-state-danger-fg", "border-state-danger-border", "bg-state-danger-bg"),
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
