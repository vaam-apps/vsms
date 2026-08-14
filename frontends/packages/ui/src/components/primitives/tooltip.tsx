import type { ReactNode } from "react";
import { cn } from "../../lib/cn";

/**
 * D5: `@radix-ui/react-tooltip` is deleted outright, replaced by DaisyUI's
 * native `.tooltip`/`data-tip` CSS component — no JS, no portal, hover/focus
 * driven by `:hover`/`:focus` in CSS alone. Headless UI ships no Tooltip
 * equivalent, and introducing a second behaviour library (e.g.
 * `@floating-ui/react`) just to replace one Radix package with another
 * contradicts constraints 6–7.
 *
 * **Accepted limitation (named explicitly in the design doc, D5):** `data-tip`
 * is a plain HTML attribute rendered via CSS `content: attr(data-tip)`, so
 * the label must be a string — no rich/interactive tooltip content anywhere
 * in this console. Nothing today needs one (`side-nav.tsx`'s own icon-rail
 * tooltips already use the identical `tooltip`/`data-tip` convention
 * directly, predating this file).
 *
 * The API collapses Radix's four-part compound
 * (`TooltipProvider`/`Tooltip`/`TooltipTrigger`/`TooltipContent`) into one
 * wrapper, since DaisyUI's mechanism needs no provider and no separate
 * trigger/content split — the trigger is just `children`.
 */
export interface TooltipProps {
  /** Plain text only — see the module doc above. */
  label: string;
  position?: "top" | "bottom" | "left" | "right";
  className?: string;
  children: ReactNode;
}

export function Tooltip({ label, position = "top", className, children }: TooltipProps) {
  return (
    <div className={cn("tooltip", `tooltip-${position}`, className)} data-tip={label}>
      {children}
    </div>
  );
}
