import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { StateMarkFromMeta } from "./state-mark";
import { HUE_CLASSES } from "./status-pill";
import { ATTEMPT_STATUS_META, type AttemptState } from "./status-tokens";

export interface AttemptStatusPillProps {
  state: AttemptState;
  /** Default `'auto'`: resolved from each state's own `attention` field. */
  variant?: "auto" | "quiet" | "loud";
  size?: "sm" | "md";
  showLiteral?: boolean;
  locale?: "en" | "fr";
  detail?: ReactNode;
  className?: string;
}

/**
 * `StatusPill`/`JobStatusPill`'s counterpart for `WebhookAttempt` (#55) —
 * same glyph/label/attention treatment (design doc §5.3), driven by
 * `ATTEMPT_STATUS_META`. A third, separate component rather than widening
 * either existing pill, for the identical reason `JobStatusPill`'s own doc
 * gives: the three state machines' meanings genuinely differ even where a
 * literal matches (`ATTEMPT_STATUS_META.failed`'s own comment — retryable,
 * not terminal, same shape `Job.failed` already has), and collapsing them
 * into one prop type would blur exactly the distinction each state
 * machine's own transitions table draws.
 */
export function AttemptStatusPill({
  state,
  variant = "auto",
  size = "sm",
  showLiteral = false,
  locale = "en",
  detail,
  className,
}: AttemptStatusPillProps) {
  const meta = ATTEMPT_STATUS_META[state];
  const resolvedVariant = variant === "auto" ? meta.attention : variant;
  const loud = resolvedVariant === "loud";
  const hue = HUE_CLASSES[meta.hue];
  const label = locale === "fr" ? meta.labelFr : meta.labelEn;
  const markSize = size === "md" ? 16 : 14;

  return (
    <span
      role="img"
      title={meta.tooltipEn}
      aria-label={`${state} — ${label}`}
      className={cn(
        "inline-flex items-center gap-[5px] whitespace-nowrap align-middle",
        "text-caption",
        loud && [hue.bg, hue.border, "rounded-sm border py-0.5 pr-1.5 pl-[5px] font-medium"],
        !loud && "gap-[5px]",
        className,
      )}
    >
      <StateMarkFromMeta meta={meta} size={markSize} className={hue.fg} />
      <span className={loud ? hue.fg : "text-muted-foreground"}>{label}</span>
      {showLiteral && <span className="font-mono text-subtle-foreground text-[11px]">{state}</span>}
      {detail != null && <span className="font-mono text-subtle-foreground">{detail}</span>}
    </span>
  );
}
