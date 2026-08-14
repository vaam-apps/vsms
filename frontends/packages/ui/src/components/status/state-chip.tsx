import { cva, type VariantProps } from "class-variance-authority";
import type { HTMLAttributes } from "react";
import { cn } from "../../lib/cn";

/**
 * A compact, state-toned chip for a single word or short phrase rendered
 * *inline* — beside a table cell's value, after a label, inside a card.
 *
 * # Not `InlineBanner`, and not `Badge`
 *
 * `InlineBanner` is the full-width `px-3 py-2` notice that owns its own
 * line. Four route groups correctly refused to route these through it
 * during the R6 factorization: forcing a chip into a banner would mean
 * overriding padding and swapping `<span>` for `<div>`, which is fighting
 * the primitive rather than using it. That refusal was right, and this is
 * the component they were missing.
 *
 * `Badge` is the other near-miss: it exists, but it is built on daisyUI's
 * `badge-neutral`/`badge-outline` classes, a different visual system from
 * the `state-*` token family every status surface in this console uses.
 * A chip that must read as *success* or *danger* cannot express that
 * through `Badge` without bypassing its variants entirely.
 *
 * # Why it is shared
 *
 * Five sites across three route groups (`workers`, `routes`, `simulator`)
 * had byte-identical `rounded-sm border border-state-<tone>-border
 * bg-state-<tone>-bg px-1.5 py-0.5 text-caption text-state-<tone>-fg`
 * strings, differing only in tone. A sixth (`dashboard`) used the same
 * shape at `px-2`. They are unified here at `px-1.5`, the majority
 * spelling — a one-step spacing change on that single site, and the same
 * normalisation `DetailRow` made for the same reason: the console should
 * not render one concept two ways because two people typed a different
 * padding.
 *
 * `StatusPill`/`JobStatusPill`/`AttemptStatusPill` are **not** replaced by
 * this. Those map a specific state machine's variants onto a fixed
 * vocabulary and carry a glyph; this is the generic chip for everything
 * that is state-toned but not one of those three machines.
 */
export const stateChipVariants = cva("rounded-sm border px-1.5 py-0.5 text-caption", {
  variants: {
    tone: {
      success: "border-state-success-border bg-state-success-bg text-state-success-fg",
      danger: "border-state-danger-border bg-state-danger-bg text-state-danger-fg",
      warning: "border-state-warning-border bg-state-warning-bg text-state-warning-fg",
      uncertain: "border-state-uncertain-border bg-state-uncertain-bg text-state-uncertain-fg",
    },
  },
  defaultVariants: { tone: "uncertain" },
});

export type StateChipTone = NonNullable<VariantProps<typeof stateChipVariants>["tone"]>;

export interface StateChipProps
  extends HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof stateChipVariants> {}

export function StateChip({ className, tone, ...props }: StateChipProps) {
  return <span className={cn(stateChipVariants({ tone }), className)} {...props} />;
}
