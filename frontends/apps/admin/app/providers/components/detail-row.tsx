import type { ReactNode } from "react";

// Route-local (R6): the labelled `dt`/`dd` row `ProviderQuickDetail`'s own
// summary `dl` repeats seven times. Not created as a shared `@vsms/ui`
// primitive — the identical shape repeats independently in
// `routes`/`webhooks`/`sender-ids` too, each factored the same way in its
// own `components/detail-row.tsx`; see that file's own doc comment
// (`routes/components/detail-row.tsx`) for why it stays route-local.
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
