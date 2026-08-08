import type { HTMLAttributes } from "react";
import { cn } from "../../lib/cn";

export type BadgeVariant = "neutral" | "outline";

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  variant?: BadgeVariant;
}

/**
 * Non-status tags only: app name, provider key, role, environment, sender
 * ID. Never use `Badge` for a message or job state — that's `StatusPill`'s
 * job (design doc §5.2); mixing them is how the status language erodes.
 */
export function Badge({ className, variant = "neutral", ...props }: BadgeProps) {
  return (
    <span
      className={cn(
        "badge rounded-sm font-mono text-caption",
        variant === "neutral" && "badge-neutral",
        variant === "outline" && "badge-outline",
        className,
      )}
      {...props}
    />
  );
}
