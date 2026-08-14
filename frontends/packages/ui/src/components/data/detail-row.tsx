import type { ReactNode } from "react";
import { cn } from "../../lib/cn";

/**
 * A labelled value in a detail drawer or summary panel — one `dt`/`dd`
 * pair, wrapped in a `<div>` so the pair can be laid out as a unit.
 *
 * # Why this is shared, when four route-local copies said it should not be
 *
 * `providers/`, `routes/`, `sender-ids/` and `webhooks/` each carry a
 * `components/detail-row.tsx` whose **code is byte-identical** — only the
 * doc comments differ. Each of those comments argues, in good faith, that
 * the component is route-local because "the identical shape repeats
 * independently in the other three, each factored the same way in its own
 * file". Four files justifying their own duplication by pointing at each
 * other is the clearest possible signal the thing belongs here instead.
 *
 * They were written by four agents working in parallel on disjoint route
 * groups, none of which could edit this package — a deliberate constraint,
 * because the round before it produced two `ScreenHeader`s and two
 * `MESSAGE_CLASSES` from agents that *could*. The trade was that
 * cross-route duplicates get reconciled centrally afterwards. This is that.
 *
 * # The variants are the console's real, pre-existing treatments
 *
 * Nothing here is a new design. Eight sites were read first and their
 * looks catalogued; the three variants below are exactly what was already
 * on screen, so every converted site keeps its current appearance:
 *
 * - `inline` — label left, value right, on one line. The default, and what
 *   the four route-local copies rendered.
 * - `stacked` — label above value. What those same copies rendered under
 *   their own `stacked` prop, for a value too long to sit beside its label.
 * - `divided` — label above value, hairline rule beneath, last row
 *   unruled. What `jobs`, `opt-outs` and `workers` each hand-rolled as a
 *   private `*DetailField` helper.
 *
 * **One deliberate normalisation, called out rather than slipped in:**
 * `audit-log` and the gallery used `flex justify-between gap-4` — the
 * `inline` treatment with a slightly wider gap and no vertical centring.
 * They map onto `inline` here, so those two drawers shift by one spacing
 * step and gain baseline centring. That is a real, if small, visual change,
 * and it is the point: the console should not render the same concept two
 * ways because two people typed a different gap.
 */
export type DetailRowVariant = "inline" | "stacked" | "divided";

export interface DetailRowProps {
  label: ReactNode;
  children: ReactNode;
  variant?: DetailRowVariant | undefined;
  className?: string | undefined;
}

export function DetailRow({ label, children, variant = "inline", className }: DetailRowProps) {
  if (variant === "divided") {
    return (
      <div
        className={cn(
          "flex flex-col gap-0.5 border-edge-subtle border-b py-2 last:border-b-0",
          className,
        )}
      >
        <dt className="text-caption text-subtle-foreground">{label}</dt>
        <dd className="text-body text-foreground">{children}</dd>
      </div>
    );
  }

  if (variant === "stacked") {
    return (
      <div className={cn("flex flex-col gap-1", className)}>
        <dt className="text-muted-foreground">{label}</dt>
        <dd className="text-caption">{children}</dd>
      </div>
    );
  }

  return (
    <div className={cn("flex items-center justify-between gap-3", className)}>
      <dt className="text-muted-foreground">{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

/**
 * The `<dl>` a group of [`DetailRow`]s belongs in.
 *
 * Exists so a caller cannot pair the rows with the wrong container — every
 * one of the eight original sites wrapped its rows in a `<dl>` with its own
 * spacing (`gap-2`, `gap-3`, or none), which is the same one-decision-in-
 * many-places problem the rows themselves had, one level up.
 *
 * `divided` rows carry their own vertical rhythm through padding and the
 * hairline rule, so the list adds no gap for them — passing `gap` there
 * would double-space rows that are already separated by a border.
 */
export function DetailList({
  children,
  variant = "inline",
  className,
}: {
  children: ReactNode;
  variant?: DetailRowVariant | undefined;
  className?: string | undefined;
}) {
  return (
    <dl className={cn("flex flex-col text-body", variant !== "divided" && "gap-3", className)}>
      {children}
    </dl>
  );
}
