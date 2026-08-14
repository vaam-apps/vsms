import type { ReactNode } from "react";

// Route-local (R6): the labelled `dt`/`dd` row every `dl`-based detail view
// on this screen repeats — `RouteQuickDetailBody` alone has five of them,
// identical apart from the label and value. Not created as a shared
// `@vsms/ui` primitive: the identical shape repeats independently across
// `webhooks`/`sender-ids`/`providers` too, each factored the same way in
// its own `components/detail-row.tsx` — same reasoning
// `error-banner.tsx`'s own doc comment gives for staying route-local rather
// than colliding with the other screen-owning agents extracting the
// identical markup from their own screens in parallel.
export function DetailList({ children }: { children: ReactNode }) {
  return <dl className="flex flex-col gap-3 text-body">{children}</dl>;
}

export interface DetailRowProps {
  label: ReactNode;
  children: ReactNode;
  /** For a value too long to sit beside its label on one line (free text,
   * a summary) — stacks label above value instead of the default
   * space-between row. */
  stacked?: boolean;
}

export function DetailRow({ label, children, stacked = false }: DetailRowProps) {
  if (stacked) {
    return (
      <div className="flex flex-col gap-1">
        <dt className="text-muted-foreground">{label}</dt>
        <dd className="text-caption">{children}</dd>
      </div>
    );
  }
  return (
    <div className="flex items-center justify-between gap-3">
      <dt className="text-muted-foreground">{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}
