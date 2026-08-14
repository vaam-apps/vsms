import { cva, type VariantProps } from "class-variance-authority";
import type { HTMLAttributes } from "react";
import { cn } from "../../lib/cn";

/**
 * Non-status tags only: app name, provider key, role, environment, sender
 * ID. Never use `Badge` for a message or job state — that's `StatusPill`'s
 * job (design doc §5.2); mixing them is how the status language erodes.
 *
 * D11: `cva()` replaces the previous `variant === "x" && "..."` conditional
 * chain, same classes. One deliberate D8 diff, not a byte-identical carry-
 * over: the previous base string hard-coded `rounded-sm`, which fought
 * daisyUI's own `.badge` rule (`border-radius: var(--radius-selector)`,
 * confirmed by reading `daisyui/components/badge.css` directly) at equal
 * specificity — a redundant override, not a deliberate choice, and for the
 * wrong tier besides: `--radius-selector` (8px) is this register's
 * small-control tier, `--radius-sm` now aliases `--radius-field` (12px).
 * Dropping the override lets daisyUI's own class govern radius, which is
 * exactly constraint 7 ("DaisyUI does the work") and the correct tier for
 * a tag-sized control.
 */
export const badgeVariants = cva("badge font-mono text-caption", {
  variants: {
    variant: {
      neutral: "badge-neutral",
      outline: "badge-outline",
    },
  },
  defaultVariants: { variant: "neutral" },
});

export type BadgeVariant = NonNullable<VariantProps<typeof badgeVariants>["variant"]>;

export interface BadgeProps
  extends HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

export function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}
