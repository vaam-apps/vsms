import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { StateMarkFromMeta } from "./state-mark";
import { HUE_CLASSES } from "./status-pill";
import { JOB_STATUS_META, type JobState } from "./status-tokens";

export interface JobStatusPillProps {
  state: JobState;
  /** Default `'auto'`: resolved from each state's own `attention` field. */
  variant?: "auto" | "quiet" | "loud";
  size?: "sm" | "md";
  showLiteral?: boolean;
  detail?: ReactNode;
  className?: string;
}

/**
 * `StatusPill`'s counterpart for `Job` (#56) — same glyph/label/attention
 * treatment (design doc §5.3), driven by `JOB_STATUS_META` instead of
 * `MESSAGE_STATUS_META`. A separate component rather than widening
 * `StatusPill` itself to a union of both state types: `StatusPill`'s own
 * `state` prop is relied on elsewhere as a `MessageState`-only signal (its
 * `aria-label`, its `variant="auto"` resolution), and the two state
 * machines' meanings genuinely differ (`JOB_STATUS_META`'s own module doc:
 * a job's `failed` is retryable, a message's `failed` is terminal) —
 * collapsing them into one prop type would blur exactly the distinction
 * that doc exists to keep visible.
 */
export function JobStatusPill({
  state,
  variant = "auto",
  size = "sm",
  showLiteral = false,
  detail,
  className,
}: JobStatusPillProps) {
  const meta = JOB_STATUS_META[state];
  const resolvedVariant = variant === "auto" ? meta.attention : variant;
  const loud = resolvedVariant === "loud";
  const hue = HUE_CLASSES[meta.hue];
  const markSize = size === "md" ? 16 : 14;

  return (
    <span
      role="img"
      title={meta.tooltip}
      aria-label={`${state} — ${meta.label}`}
      className={cn(
        "inline-flex items-center gap-[5px] whitespace-nowrap align-middle",
        "text-caption",
        loud && [hue.bg, hue.border, "rounded-sm border py-0.5 pr-1.5 pl-[5px] font-medium"],
        !loud && "gap-[5px]",
        className,
      )}
    >
      <StateMarkFromMeta meta={meta} size={markSize} className={hue.fg} />
      <span className={loud ? hue.fg : "text-muted-foreground"}>{meta.label}</span>
      {showLiteral && <span className="font-mono text-subtle-foreground text-[11px]">{state}</span>}
      {detail != null && <span className="font-mono text-subtle-foreground">{detail}</span>}
    </span>
  );
}
