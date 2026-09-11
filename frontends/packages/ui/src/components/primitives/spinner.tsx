import { cn } from "../../lib/cn";

export interface SpinnerProps {
  size?: "xs" | "sm" | "md" | "lg";
  /** Announced to assistive technology. `undefined` renders the spinner
   * `aria-hidden`, which is correct when a visible sibling already says
   * what is loading — two announcements of the same wait is worse than
   * one. */
  label?: string | undefined;
  className?: string | undefined;
}

const SIZE_CLASS = {
  xs: "loading-xs",
  sm: "loading-sm",
  md: "loading-md",
  lg: "loading-lg",
} as const;

/**
 * An indeterminate busy indicator, for a wait whose *shape* is unknown.
 *
 * Prefer `Skeleton` wherever the shape of what is arriving is known —
 * a table, a card, a detail pane. A skeleton keeps the layout from
 * jumping when content lands and tells the reader what to expect; a
 * spinner tells them only that something is happening. This is for the
 * rest: a button mid-submit, a poll with no placeholder to draw.
 */
export function Spinner({ size = "sm", label, className }: SpinnerProps) {
  return (
    <span
      className={cn("loading loading-spinner", SIZE_CLASS[size], className)}
      role={label === undefined ? undefined : "status"}
      aria-hidden={label === undefined ? true : undefined}
    >
      {label !== undefined && <span className="sr-only">{label}</span>}
    </span>
  );
}
