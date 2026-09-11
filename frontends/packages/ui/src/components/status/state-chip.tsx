import type { HTMLAttributes } from "react";
import { cn } from "../../lib/cn";
import { HUE_CLASSES, type StatusHue } from "./status-tokens";

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
 * `StatusPill` is **not** replaced by this. It maps a whole state
 * machine's variants onto a fixed vocabulary and carries a glyph; this is
 * the generic chip for everything that is state-toned but is not a state
 * machine — a capability flag, a derived verdict, a one-word qualifier.
 *
 * # Tone
 *
 * Tone comes from the shared [`StatusHue`] vocabulary rather than a second
 * private list. Before this, `StateChip` carried its own four-tone table
 * whose class strings were transcribed by hand from the same tokens
 * `HUE_CLASSES` already held — two copies of one mapping, and the chip's
 * copy was missing three of the hues, so a caller wanting `expired` or
 * `parked` had no way to ask for it and reached for `className` instead.
 */
export type StateChipTone = StatusHue;

export interface StateChipProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: StateChipTone | undefined;
}

export function StateChip({ className, tone = "uncertain", ...props }: StateChipProps) {
  const hue = HUE_CLASSES[tone];
  return (
    <span
      className={cn(
        "rounded-sm border px-1.5 py-0.5 text-caption",
        hue.border,
        hue.bg,
        hue.fg,
        className,
      )}
      {...props}
    />
  );
}
